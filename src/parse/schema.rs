//! `components/schemas` to structs and enums, and the OpenAPI to Rust type map.

use std::collections::HashSet;

use openapiv3::{
    IntegerFormat, ReferenceOr, Schema, SchemaKind, StringFormat, Type, VariantOrUnknownOrEmpty,
};

use super::{Parser, constraint::extract_constraints};
use crate::{
    Constraints, Entity, EntityKind, EnumDef, Field, StructDef,
    diagnostic::Severity,
    ident::{to_type_ident, to_variant_ident},
};

impl Parser {
    pub(super) fn parse_schema(&mut self, name: &str, schema: &Schema) -> Option<Entity> {
        let SchemaKind::Type(Type::Object(obj)) = &schema.schema_kind else {
            return None;
        };

        // A map-shaped object (`additionalProperties` with an empty property set) is
        // generated as an empty struct, silently dropping the map value type.
        if has_meaningful_additional_properties(obj) {
            self.record(
                Severity::Degraded,
                format!("components.schemas.{name}"),
                "additionalProperties",
                "additionalProperties is ignored; map values are not represented in the generated struct",
            );
        }

        let mut fields = Vec::new();
        let mut enums = Vec::new();

        for (field_name, field_ref) in &obj.properties {
            let required = obj.required.contains(field_name);
            let context = format!("{name}.{field_name}");

            // Check for inline string enums. `is_inline_enum` is recorded per field
            // rather than re-derived from the name later: a `$ref` field pointing at
            // a schema that happens to share an inline enum's name is NOT an enum
            // field, and conflating the two silently mistypes it.
            let (rust_type, nullable, is_inline_enum) = match field_ref {
                ReferenceOr::Item(field_schema) => {
                    if let Some(enum_def) = self.parse_enum(field_name, field_schema, &context) {
                        let ty = enum_def.name.clone();
                        let nullable = field_schema.schema_data.nullable;
                        enums.push(enum_def);
                        (ty, nullable, true)
                    } else {
                        let (ty, nullable) = self.map_schema_to_type(field_schema, &context);
                        (ty, nullable, false)
                    }
                }
                ReferenceOr::Reference { .. } => {
                    let (ty, nullable) = self.resolve_field_type(field_ref, &context);
                    (ty, nullable, false)
                }
            };

            // An inline enum's values are what its default has to name.
            let enum_values: Option<Vec<String>> = is_inline_enum.then(|| {
                enums
                    .last()
                    .map(|e| {
                        e.variants
                            .iter()
                            .map(|(_, original)| original.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            });

            let default_value = match field_ref {
                ReferenceOr::Item(field_schema) => self.extract_default(
                    &field_schema.schema_data.default,
                    &rust_type,
                    enum_values.as_deref(),
                    &context,
                ),
                ReferenceOr::Reference { .. } => None,
            };

            let has_default = default_value.is_some();
            let is_optional = if has_default {
                nullable
            } else {
                !required || nullable
            };

            // If the field was converted to an enum type, serde handles validation,
            // no runtime constraints needed.
            let constraints = if is_inline_enum {
                Constraints::None
            } else {
                match field_ref {
                    ReferenceOr::Reference { .. } => Constraints::Nested,
                    ReferenceOr::Item(field_schema) => {
                        if let SchemaKind::Type(Type::Array(arr)) = &field_schema.schema_kind {
                            if let Some(ReferenceOr::Reference { .. }) = &arr.items {
                                Constraints::VecNested
                            } else {
                                extract_constraints(field_schema)
                            }
                        } else {
                            extract_constraints(field_schema)
                        }
                    }
                }
            };

            fields.push(Field {
                name: field_name.clone(),
                rust_type,
                ref_target: field_ref_target(field_ref),
                is_optional,
                constraints,
                default_value,
                is_inline_enum,
            });
        }

        Some(Entity::Struct(StructDef {
            name: to_type_ident(name)?,
            kind: EntityKind::Schema,
            fields,
            enums,
        }))
    }

    /// If the schema is a string with enum values, generate an EnumDef.
    ///
    /// Enum values are arbitrary strings (`10min`, `a.b`, `""`), so every variant
    /// name goes through [`to_variant_ident`]; the original value is kept for the
    /// `#[serde(rename)]` that preserves the wire format.
    ///
    /// Sanitizing can map two distinct values onto one name: `a.b` and `a-b` both
    /// give `AB`, and a value that needed the `Variant` prefix can land on one that
    /// spells it out (`10min` and `Variant10min`). There is no good answer at that
    /// point: dropping a value makes it undeserializable, and suffixing one puts a
    /// variant in the generated API that appears nowhere in the spec, so the
    /// collision is [`Severity::Fatal`] and generation fails. The colliding variant
    /// is still emitted (uncompilable rather than quietly wrong) for callers that
    /// go through [`parse`] and ignore diagnostics.
    pub(super) fn parse_enum(
        &mut self,
        name: &str,
        schema: &Schema,
        context: &str,
    ) -> Option<EnumDef> {
        let SchemaKind::Type(Type::String(s)) = &schema.schema_kind else {
            return None;
        };
        let values: Vec<String> = s.enumeration.iter().filter_map(Clone::clone).collect();
        if values.is_empty() {
            return None;
        }

        let mut seen_values = HashSet::new();
        // Variant name → the first value that claimed it, for the collision report.
        let mut taken_names: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut variants: Vec<(String, String)> = Vec::new();
        for value in values {
            // A repeated value would emit two variants renamed to the same string,
            // which serde rejects at compile time.
            if !seen_values.insert(value.clone()) {
                self.record(
                    Severity::Dropped,
                    context.to_string(),
                    "enum value",
                    format!("value `{value}` is listed twice; the repeat was dropped"),
                );
                continue;
            }

            let Some(variant) = to_variant_ident(&value) else {
                self.record(
                    Severity::Fatal,
                    context.to_string(),
                    "enum value",
                    format!("enum value \"{value}\" has nothing a Rust name can be built from"),
                );
                continue;
            };
            if let Some(first) = taken_names.get(&variant) {
                self.record(
                    Severity::Fatal,
                    context.to_string(),
                    "enum value",
                    format!(
                        "enum values \"{first}\" and \"{value}\" would both become \
                         the Rust enum variant `{variant}`"
                    ),
                );
            } else {
                taken_names.insert(variant.clone(), value.clone());
            }
            variants.push((variant, value));
        }

        Some(EnumDef {
            name: to_type_ident(name)?,
            variants,
        })
    }

    /// Record a diagnostic for a top-level schema whose kind the generator does not
    /// turn into a type. `oneOf` member-level losses are reported inside
    /// [`parse_one_of`]; this covers the schema-level drop.
    pub(super) fn diagnose_unsupported_schema(&mut self, name: &str, schema: &Schema) {
        let path = format!("components.schemas.{name}");
        let (construct, reason): (&str, &str) = match &schema.schema_kind {
            SchemaKind::AllOf { .. } => (
                "allOf",
                "allOf composition is not supported; no type was generated",
            ),
            SchemaKind::AnyOf { .. } => (
                "anyOf",
                "anyOf composition is not supported; no type was generated",
            ),
            SchemaKind::Not { .. } => (
                "not",
                "`not` schemas are not supported; no type was generated",
            ),
            SchemaKind::Any(_) => (
                "free-form schema",
                "free-form/ambiguous schema; no type was generated",
            ),
            SchemaKind::OneOf { .. } => (
                "oneOf",
                "oneOf has no usable local $ref members; no type was generated",
            ),
            SchemaKind::Type(Type::String(_)) => (
                "string schema",
                "top-level string type alias (or non-string enum) is not generated as a distinct type",
            ),
            SchemaKind::Type(Type::Integer(_)) => (
                "integer schema",
                "top-level integer schema (including integer enums) is not generated as a distinct type",
            ),
            SchemaKind::Type(Type::Number(_)) => (
                "number schema",
                "top-level number type alias is not generated as a distinct type",
            ),
            SchemaKind::Type(Type::Boolean(_)) => (
                "boolean schema",
                "top-level boolean type alias is not generated as a distinct type",
            ),
            SchemaKind::Type(Type::Array(_)) => (
                "array schema",
                "top-level array type alias is not generated as a distinct type",
            ),
            // Objects are always handled by `parse_schema`; nothing to report.
            SchemaKind::Type(Type::Object(_)) => return,
        };
        self.record(Severity::Dropped, path, construct, reason);
    }

    /// Map an OpenAPI schema to a Rust type string and nullable flag.
    ///
    /// Handles string (with date-time format), integer (i32/i64), number (f64),
    /// boolean, and array types. Falls back to `serde_json::Value` for anything
    /// else, recording a [`Severity::Degraded`] diagnostic at `context`.
    fn map_schema_to_type(&mut self, schema: &Schema, context: &str) -> (String, bool) {
        let nullable = schema.schema_data.nullable;

        match &schema.schema_kind {
            SchemaKind::Type(Type::String(s)) => {
                let ty = match &s.format {
                    VariantOrUnknownOrEmpty::Item(StringFormat::DateTime) => "DateTime<Utc>",
                    VariantOrUnknownOrEmpty::Item(StringFormat::Date) => "NaiveDate",
                    VariantOrUnknownOrEmpty::Unknown(f) if f == "uuid" => "Uuid",
                    _ => "String",
                };
                (ty.to_string(), nullable)
            }
            SchemaKind::Type(Type::Integer(i)) => {
                let ty = match i.format {
                    VariantOrUnknownOrEmpty::Item(IntegerFormat::Int32) => "i32",
                    _ => "i64",
                };
                (ty.to_string(), nullable)
            }
            SchemaKind::Type(Type::Number(_)) => ("f64".to_string(), nullable),
            SchemaKind::Type(Type::Boolean(_)) => ("bool".to_string(), nullable),
            SchemaKind::Type(Type::Array(arr)) => {
                let inner = match &arr.items {
                    Some(ref_or) => {
                        let (t, _) = self.resolve_field_type(ref_or, context);
                        t
                    }
                    None => {
                        self.record(
                            Severity::Degraded,
                            context.to_string(),
                            "array without items",
                            "array has no `items` schema; element type is `serde_json::Value`",
                        );
                        "serde_json::Value".to_string()
                    }
                };
                (format!("Vec<{inner}>"), nullable)
            }
            other => {
                self.record(
                    Severity::Degraded,
                    context.to_string(),
                    describe_field_kind(other),
                    "field type is not supported; generated as `serde_json::Value`",
                );
                ("serde_json::Value".to_string(), nullable)
            }
        }
    }

    /// Resolve a field's `ReferenceOr<Schema>` to a `(rust_type, nullable)` pair.
    fn resolve_field_type(
        &mut self,
        field_ref: &ReferenceOr<Box<Schema>>,
        context: &str,
    ) -> (String, bool) {
        match field_ref {
            ReferenceOr::Reference { reference } => {
                self.diagnose_ref(reference, context);
                (self.ref_type_name(reference, context), false)
            }
            ReferenceOr::Item(schema) => self.map_schema_to_type(schema, context),
        }
    }

    /// Resolve a schema reference (used for parameters) to a `(rust_type, nullable)` pair.
    pub(super) fn resolve_schema_ref(
        &mut self,
        schema_ref: &ReferenceOr<Schema>,
        context: &str,
    ) -> (String, bool) {
        match schema_ref {
            ReferenceOr::Reference { reference } => {
                self.diagnose_ref(reference, context);
                (self.ref_type_name(reference, context), false)
            }
            ReferenceOr::Item(schema) => self.map_schema_to_type(schema, context),
        }
    }
}

/// The `$ref` a field's type comes from: the field's own, or its array items'.
fn field_ref_target(field_ref: &ReferenceOr<Box<Schema>>) -> Option<String> {
    match field_ref {
        ReferenceOr::Reference { reference } => Some(reference.clone()),
        ReferenceOr::Item(schema) => match &schema.schema_kind {
            SchemaKind::Type(Type::Array(arr)) => match arr.items.as_ref()? {
                ReferenceOr::Reference { reference } => Some(reference.clone()),
                ReferenceOr::Item(_) => None,
            },
            _ => None,
        },
    }
}

/// Whether an object schema carries an `additionalProperties` that we drop
/// (a schema value or `true`). `additionalProperties: false` carries no data.
fn has_meaningful_additional_properties(obj: &openapiv3::ObjectType) -> bool {
    matches!(
        obj.additional_properties,
        Some(openapiv3::AdditionalProperties::Schema(_))
            | Some(openapiv3::AdditionalProperties::Any(true))
    )
}

/// A short label for an unsupported inline field schema kind, for diagnostics.
fn describe_field_kind(kind: &SchemaKind) -> &'static str {
    match kind {
        SchemaKind::Type(Type::Object(_)) => "inline object",
        SchemaKind::OneOf { .. } => "inline oneOf",
        SchemaKind::AllOf { .. } => "inline allOf",
        SchemaKind::AnyOf { .. } => "inline anyOf",
        SchemaKind::Not { .. } => "inline not",
        SchemaKind::Any(_) => "inline free-form schema",
        // The concrete scalar/array types are handled by `map_schema_to_type`;
        // this arm exists only for exhaustiveness.
        SchemaKind::Type(_) => "unsupported schema",
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{
        Constraints, Entity, EntityKind, EnumDef, Field, StructDef, diagnostic::Severity,
        load_spec, parse, parse::testutil::*,
    };

    #[test]
    fn parse_schema_entity() -> Result<()> {
        let yaml = format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      required: [id]
      properties:
        id:
          type: integer
          format: int64
        name:
          type: string
"
        );
        let (entities, _) = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0],
            Entity::Struct(StructDef {
                name: String::from("Foo"),
                kind: EntityKind::Schema,
                fields: vec![
                    Field {
                        name: "id".into(),
                        rust_type: "i64".into(),
                        is_optional: false,
                        constraints: Constraints::None,
                        default_value: None,
                        ref_target: None,
                        is_inline_enum: false,
                    },
                    Field {
                        name: "name".into(),
                        rust_type: "String".into(),
                        is_optional: true,
                        constraints: Constraints::None,
                        default_value: None,
                        ref_target: None,
                        is_inline_enum: false,
                    },
                ],
                enums: vec![],
            })
        );

        Ok(())
    }

    #[test]
    fn parse_standalone_enum() -> Result<()> {
        let yaml = format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Status:
      type: string
      enum: [ACTIVE, INACTIVE, PENDING]
"
        );
        let (entities, _) = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0],
            Entity::Enum(EnumDef {
                name: "Status".into(),
                variants: vec![
                    ("ACTIVE".into(), "ACTIVE".into()),
                    ("INACTIVE".into(), "INACTIVE".into()),
                    ("PENDING".into(), "PENDING".into()),
                ],
            })
        );

        Ok(())
    }

    /// A `$ref` field whose target schema shares an inline enum's name is not an
    /// enum field: it must keep the referenced type and still validate nested.
    #[test]
    fn ref_field_not_confused_with_inline_enum() -> Result<()> {
        let spec = load_spec(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Kind:
      type: object
      required: [name]
      properties:
        name:
          type: string
          minLength: 5
    Foo:
      type: object
      required: [kind, other]
      properties:
        kind:
          type: string
          enum: [x, y]
        other:
          $ref: '#/components/schemas/Kind'
"
        ))?;
        let (entities, _) = parse(&spec);
        let foo = find_struct(&entities, "Foo");

        let kind = &foo.fields[0];
        assert!(kind.is_inline_enum);
        assert_eq!(kind.rust_type, "Kind", "inline enum keeps its raw name");

        let other = &foo.fields[1];
        assert!(!other.is_inline_enum, "a $ref field is not an inline enum");
        assert_eq!(other.rust_type, "Kind");
        assert_eq!(
            other.constraints,
            Constraints::Nested,
            "the $ref field must still validate its target"
        );

        Ok(())
    }

    #[test]
    fn diagnostic_all_of_dropped() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
    Derived:
      allOf:
        - $ref: '#/components/schemas/Base'
"
        ))?;
        let d = find_diag(&diags, "allOf");
        assert_eq!(d.severity, Severity::Dropped);
        assert_eq!(d.path, "components.schemas.Derived");

        Ok(())
    }

    #[test]
    fn diagnostic_inline_object_field_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Pet:
      type: object
      properties:
        metadata:
          type: object
          properties:
            key:
              type: string
"
        ))?;
        let d = find_diag(&diags, "inline object");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "Pet.metadata");

        Ok(())
    }

    #[test]
    fn diagnostic_external_ref_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r#"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Order:
      type: object
      properties:
        customer:
          $ref: 'other.yaml#/components/schemas/Customer'
"#
        ))?;
        let d = find_diag(&diags, "external $ref");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "Order.customer");

        Ok(())
    }

    #[test]
    fn diagnostic_additional_properties_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Metadata:
      type: object
      additionalProperties:
        type: string
"
        ))?;
        let d = find_diag(&diags, "additionalProperties");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "components.schemas.Metadata");

        Ok(())
    }
}
