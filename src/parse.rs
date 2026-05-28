use openapiv3::{
    IntegerFormat, OpenAPI, Operation, ReferenceOr, Schema, SchemaKind, StringFormat, Type,
    VariantOrUnknownOrEmpty,
};

use crate::{Constraints, Entity, EntityKind, EnumDef, Field, StructDef};

/// Parse an OpenAPI spec into a list of entities.
pub fn parse(spec: &OpenAPI) -> Vec<Entity> {
    let mut entities = Vec::new();

    if let Some(components) = &spec.components {
        entities.extend(
            components
                .schemas
                .iter()
                .filter_map(|(name, ref_or)| match ref_or {
                    ReferenceOr::Item(schema) => Some((name, schema)),
                    _ => None,
                })
                .filter_map(|(name, schema)| {
                    parse_schema(name, schema)
                        .or_else(|| parse_enum(name, schema).map(Entity::Enum))
                }),
        );
    }

    for (_path, path_item_ref) in spec.paths.iter() {
        let path_item = match path_item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { .. } => continue,
        };

        let ops = [
            &path_item.get,
            &path_item.put,
            &path_item.post,
            &path_item.patch,
            &path_item.delete,
        ];
        for op in ops.into_iter().flatten() {
            if let Some(entity) = parse_query(op, spec.components.as_ref()) {
                entities.push(entity);
            }
        }
    }

    entities
}

fn parse_schema(name: &str, schema: &Schema) -> Option<Entity> {
    let SchemaKind::Type(Type::Object(obj)) = &schema.schema_kind else {
        return None;
    };

    let mut fields = Vec::new();
    let mut enums = Vec::new();

    for (field_name, field_ref) in &obj.properties {
        let required = obj.required.contains(field_name);

        // Check for inline string enums
        let (rust_type, nullable) = match field_ref {
            ReferenceOr::Item(field_schema) => {
                if let Some(enum_def) = parse_enum(field_name, field_schema) {
                    let ty = enum_def.name.clone();
                    let nullable = field_schema.schema_data.nullable;
                    enums.push(enum_def);
                    (ty, nullable)
                } else {
                    map_schema_to_type(field_schema)
                }
            }
            ReferenceOr::Reference { .. } => resolve_field_type(field_ref),
        };

        let is_optional = !required || nullable;

        // If the field was converted to an enum type, serde handles validation —
        // no runtime constraints needed.
        let is_enum_field = enums.iter().any(|e| e.name == rust_type);

        let constraints = if is_enum_field {
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
            is_optional,
            constraints,
        });
    }

    Some(Entity::Struct(StructDef {
        name: name.to_string(),
        kind: EntityKind::Schema,
        fields,
        enums,
    }))
}

/// If the schema is a string with enum values, generate an EnumDef.
fn parse_enum(field_name: &str, schema: &Schema) -> Option<EnumDef> {
    if let SchemaKind::Type(Type::String(s)) = &schema.schema_kind {
        let values: Vec<String> = s.enumeration.iter().filter_map(Clone::clone).collect();
        if values.is_empty() {
            return None;
        }
        let enum_name = to_pascal_case(field_name);
        let variants = values
            .into_iter()
            .map(|v| {
                let pascal = to_pascal_case(&v);
                (pascal, v)
            })
            .collect();
        Some(EnumDef {
            name: enum_name,
            variants,
        })
    } else {
        None
    }
}

fn parse_query(op: &Operation, components: Option<&openapiv3::Components>) -> Option<Entity> {
    let query_params: Vec<_> = op
        .parameters
        .iter()
        .filter_map(|p| match p {
            ReferenceOr::Item(param) => Some(param),
            ReferenceOr::Reference { reference } => resolve_parameter_ref(reference, components),
        })
        .filter(|p| matches!(p, openapiv3::Parameter::Query { .. }))
        .collect();

    if query_params.is_empty() {
        return None;
    }

    let operation_id = op.operation_id.as_ref()?;
    let struct_name = format!("{}Query", to_pascal_case(operation_id));

    let mut fields = Vec::new();

    for param in &query_params {
        let data = parameter_data(param);
        let openapiv3::ParameterSchemaOrContent::Schema(schema_ref) = &data.format else {
            continue;
        };
        let (rust_type, nullable) = resolve_schema_ref(schema_ref);
        let is_optional = !data.required || nullable;

        let constraints = match schema_ref {
            ReferenceOr::Reference { .. } => Constraints::Nested,
            ReferenceOr::Item(schema) => extract_constraints(schema),
        };

        fields.push(Field {
            name: data.name.clone(),
            rust_type,
            is_optional,
            constraints,
        });
    }

    Some(Entity::Struct(StructDef {
        name: struct_name,
        kind: EntityKind::Query,
        fields,
        enums: Vec::new(),
    }))
}

/// Extract validation constraints from an inline schema.
fn extract_constraints(schema: &Schema) -> Constraints {
    match &schema.schema_kind {
        SchemaKind::Type(Type::String(s)) => {
            let enumeration: Vec<String> = s.enumeration.iter().filter_map(Clone::clone).collect();
            if s.min_length.is_none()
                && s.max_length.is_none()
                && s.pattern.is_none()
                && enumeration.is_empty()
            {
                return Constraints::None;
            }
            Constraints::String {
                min_length: s.min_length,
                max_length: s.max_length,
                pattern: s.pattern.clone(),
                enumeration,
            }
        }
        SchemaKind::Type(Type::Integer(i)) => {
            let enumeration: Vec<i64> = i.enumeration.iter().filter_map(|v| *v).collect();
            if i.minimum.is_none()
                && i.maximum.is_none()
                && !i.exclusive_minimum
                && !i.exclusive_maximum
                && i.multiple_of.is_none()
                && enumeration.is_empty()
            {
                return Constraints::None;
            }
            Constraints::Integer {
                minimum: i.minimum,
                maximum: i.maximum,
                exclusive_minimum: i.exclusive_minimum,
                exclusive_maximum: i.exclusive_maximum,
                multiple_of: i.multiple_of,
                enumeration,
            }
        }
        SchemaKind::Type(Type::Number(n)) => {
            if n.minimum.is_none()
                && n.maximum.is_none()
                && !n.exclusive_minimum
                && !n.exclusive_maximum
                && n.multiple_of.is_none()
            {
                return Constraints::None;
            }
            Constraints::Number {
                minimum: n.minimum,
                maximum: n.maximum,
                exclusive_minimum: n.exclusive_minimum,
                exclusive_maximum: n.exclusive_maximum,
                multiple_of: n.multiple_of,
            }
        }
        SchemaKind::Type(Type::Array(a)) => {
            if a.min_items.is_none() && a.max_items.is_none() && !a.unique_items {
                return Constraints::None;
            }
            Constraints::Array {
                min_items: a.min_items,
                max_items: a.max_items,
                unique_items: a.unique_items,
            }
        }
        _ => Constraints::None,
    }
}

/// Extract the common `ParameterData` from any parameter variant (query, header, path, cookie).
fn parameter_data(param: &openapiv3::Parameter) -> &openapiv3::ParameterData {
    match param {
        openapiv3::Parameter::Query { parameter_data, .. }
        | openapiv3::Parameter::Header { parameter_data, .. }
        | openapiv3::Parameter::Path { parameter_data, .. }
        | openapiv3::Parameter::Cookie { parameter_data, .. } => parameter_data,
    }
}

/// Resolve a `$ref` string like `#/components/parameters/Foo` to the actual parameter.
fn resolve_parameter_ref<'a>(
    reference: &str,
    components: Option<&'a openapiv3::Components>,
) -> Option<&'a openapiv3::Parameter> {
    let name = reference.strip_prefix("#/components/parameters/")?;
    let params = &components.as_ref()?.parameters;
    match params.get(name)? {
        ReferenceOr::Item(p) => Some(p),
        ReferenceOr::Reference { .. } => None,
    }
}

/// Extract the schema name from a `$ref` string (e.g. `#/components/schemas/Foo` -> `Foo`).
fn resolve_ref_name(reference: &str) -> String {
    reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or(reference)
        .to_string()
}

/// Resolve a field's `ReferenceOr<Schema>` to a `(rust_type, nullable)` pair.
fn resolve_field_type(field_ref: &ReferenceOr<Box<Schema>>) -> (String, bool) {
    match field_ref {
        ReferenceOr::Reference { reference } => (resolve_ref_name(reference), false),
        ReferenceOr::Item(schema) => map_schema_to_type(schema),
    }
}

/// Resolve a schema reference (used for parameters) to a `(rust_type, nullable)` pair.
fn resolve_schema_ref(schema_ref: &ReferenceOr<Schema>) -> (String, bool) {
    match schema_ref {
        ReferenceOr::Reference { reference } => (resolve_ref_name(reference), false),
        ReferenceOr::Item(schema) => map_schema_to_type(schema),
    }
}

/// Map an OpenAPI schema to a Rust type string and nullable flag.
///
/// Handles string (with date-time format), integer (i32/i64), number (f64),
/// boolean, and array types. Falls back to `serde_json::Value` for anything else.
fn map_schema_to_type(schema: &Schema) -> (String, bool) {
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
                    let (t, _) = resolve_field_type(ref_or);
                    t
                }
                None => "serde_json::Value".to_string(),
            };
            (format!("Vec<{inner}>"), nullable)
        }
        _ => ("serde_json::Value".to_string(), nullable),
    }
}

/// Convert a `snake_case` or `kebab-case` string to `PascalCase`.
pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use openapiv3::OpenAPI;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::{Constraints, Entity, EntityKind, Field, StructDef, load_spec};

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    const MINIMAL_HEADER: &str = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
"#;

    fn spec_with_schema(schema_yaml: &str) -> Result<OpenAPI> {
        let indented: String = schema_yaml
            .lines()
            .map(|line| {
                if line.trim().is_empty() {
                    String::new()
                } else {
                    format!("      {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let full =
            format!("{MINIMAL_HEADER}paths: {{}}\ncomponents:\n  schemas:\n    Foo:\n{indented}\n");
        Ok(load_spec(&full)?)
    }

    fn first_struct_fields(entities: &[Entity]) -> &[Field] {
        match &entities[0] {
            Entity::Struct(s) => &s.fields,
            _ => panic!("expected Entity::Struct"),
        }
    }

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
        let entities = parse(&load_spec(&yaml)?);
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
                    },
                    Field {
                        name: "name".into(),
                        rust_type: "String".into(),
                        is_optional: true,
                        constraints: Constraints::None,
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
        let entities = parse(&load_spec(&yaml)?);
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

    #[test]
    fn parse_query_entity() -> Result<()> {
        let yaml = format!(
            r#"{MINIMAL_HEADER}
paths:
  /things:
    get:
      operationId: getThings
      parameters:
        - name: limit
          in: query
          required: false
          schema:
            type: integer
            format: int32
        - name: id
          in: path
          required: true
          schema:
            type: integer
            format: int64
      responses:
        "200":
          description: OK
components:
  schemas: {{}}
"#
        );
        let entities = parse(&load_spec(&yaml)?);
        assert_eq!(entities.len(), 1);
        let Entity::Struct(s) = &entities[0] else {
            panic!("expected Entity::Struct");
        };
        assert_eq!(s.name, "GetThingsQuery");
        assert_eq!(s.kind, EntityKind::Query);
        assert_eq!(s.fields.len(), 1, "path param should be excluded");
        assert_eq!(s.fields[0].name, "limit");

        Ok(())
    }

    #[test]
    fn extract_constraints() -> Result<()> {
        // String pattern
        let spec = spec_with_schema(
            "\
type: object
required: [s]
properties:
  s:
    type: string
    pattern: '^[A-Z]{3}$'",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec))[0].constraints,
            Constraints::String {
                min_length: None,
                max_length: None,
                pattern: Some("^[A-Z]{3}$".into()),
                enumeration: vec![],
            }
        );

        // Integer multipleOf + enum
        let spec = spec_with_schema(
            "\
type: object
required: [v]
properties:
  v:
    type: integer
    format: int64
    multipleOf: 5
    enum: [5, 10, 15]",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec))[0].constraints,
            Constraints::Integer {
                minimum: None,
                maximum: None,
                exclusive_minimum: false,
                exclusive_maximum: false,
                multiple_of: Some(5),
                enumeration: vec![5, 10, 15],
            }
        );

        // Number exclusive min/max
        let spec = spec_with_schema(
            "\
type: object
required: [n]
properties:
  n:
    type: number
    minimum: 0.0
    maximum: 100.0
    exclusiveMinimum: true
    exclusiveMaximum: true",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec))[0].constraints,
            Constraints::Number {
                minimum: Some(0.0),
                maximum: Some(100.0),
                exclusive_minimum: true,
                exclusive_maximum: true,
                multiple_of: None,
            }
        );

        // Array uniqueItems
        let spec = spec_with_schema(
            "\
type: object
required: [tags]
properties:
  tags:
    type: array
    items:
      type: string
    uniqueItems: true",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec))[0].constraints,
            Constraints::Array {
                min_items: None,
                max_items: None,
                unique_items: true,
            }
        );

        // No constraints
        let spec = spec_with_schema(
            "\
type: object
required: [name]
properties:
  name:
    type: string",
        )?;
        assert_eq!(
            first_struct_fields(&parse(&spec))[0].constraints,
            Constraints::None
        );

        // Type mapping: boolean → bool
        let spec = spec_with_schema(
            "\
type: object
required: [active]
properties:
  active:
    type: boolean",
        )?;
        assert_eq!(first_struct_fields(&parse(&spec))[0].rust_type, "bool");

        // Type mapping: number → f64
        let spec = spec_with_schema(
            "\
type: object
required: [score]
properties:
  score:
    type: number",
        )?;
        assert_eq!(first_struct_fields(&parse(&spec))[0].rust_type, "f64");

        Ok(())
    }
}
