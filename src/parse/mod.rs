//! OpenAPI spec to [`Entity`], with a [`Diagnostic`] for every construct that
//! is dropped or degraded.
//!
//! [`Parser`] walks the spec and collects diagnostics; the submodules hold one
//! area of the walk each, and reach back for what they share: the collector,
//! the `$ref` helpers, and [`Parser::diagnose_ref`].

mod constraint;
mod default;
mod one_of;
mod operation;
mod schema;
#[cfg(test)]
mod testutil;

use std::collections::HashSet;

use openapiv3::{OpenAPI, Operation, ReferenceOr};

use crate::{
    Diagnostic, Entity,
    diagnostic::{Severity, record},
    ident::to_type_ident,
};

/// Parse an OpenAPI spec into a list of entities, plus a [`Diagnostic`] for
/// every construct that is dropped or degraded.
///
/// A caller with no use for the diagnostics discards them at the call site
/// (`let (entities, _) = parse(spec)`), which is visible where it happens.
pub fn parse(spec: &OpenAPI) -> (Vec<Entity>, Vec<Diagnostic>) {
    let mut parser = Parser::default();
    let entities = parser.entities(spec);
    (entities, parser.diagnostics)
}

/// Collects diagnostics while it walks the spec.
///
/// The collector is the receiver rather than an out-parameter, so no function
/// below carries a `&mut Vec<Diagnostic>` and none of them can drop a
/// diagnostic by forgetting to merge one.
#[derive(Default)]
struct Parser {
    diagnostics: Vec<Diagnostic>,
}

impl Parser {
    fn record(
        &mut self,
        severity: Severity,
        path: impl Into<String>,
        construct: impl Into<String>,
        reason: impl Into<String>,
    ) {
        record(&mut self.diagnostics, severity, path, construct, reason);
    }

    /// Walk the spec and build every entity it yields.
    fn entities(&mut self, spec: &OpenAPI) -> Vec<Entity> {
        let mut entities = Vec::new();

        if let Some(components) = &spec.components {
            // Schema names are sanitized into Rust type names, so two schemas can
            // land on the same one (`foo-bar` and `foo_bar` both give `FooBar`).
            // Neither generating both nor picking one is defensible, since the
            // `$ref`s to them are indistinguishable, so the clash is fatal.
            let mut type_names: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for (name, ref_or) in &components.schemas {
                let schema = match ref_or {
                    ReferenceOr::Item(schema) => schema,
                    ReferenceOr::Reference { reference } => {
                        self.record(
                            Severity::Dropped,
                            format!("components.schemas.{name}"),
                            "$ref schema",
                            format!(
                                "top-level schema is a $ref to `{reference}`; no type was generated"
                            ),
                        );
                        continue;
                    }
                };

                let Some(type_name) = to_type_ident(name) else {
                    self.record(
                        Severity::Fatal,
                        format!("components.schemas.{name}"),
                        "schema",
                        format!("schema name \"{name}\" has nothing a Rust name can be built from"),
                    );
                    continue;
                };
                if let Some(first) = type_names.get(&type_name) {
                    self.record(
                        Severity::Fatal,
                        format!("components.schemas.{name}"),
                        "schema",
                        format!(
                            "schemas \"{first}\" and \"{name}\" would both become \
                             the Rust type `{type_name}`"
                        ),
                    );
                } else {
                    type_names.insert(type_name.clone(), name.clone());
                }
                let entity = self
                    .parse_schema(name, schema)
                    .or_else(|| self.parse_one_of(name, schema))
                    .or_else(|| {
                        self.parse_enum(name, schema, &format!("components.schemas.{name}"))
                            .map(Entity::Enum)
                    });

                match entity {
                    Some(entity) => entities.push(entity),
                    None => self.diagnose_unsupported_schema(name, schema),
                }
            }
        }

        for (path, path_item_ref) in spec.paths.iter() {
            let path_item = match path_item_ref {
                ReferenceOr::Item(item) => item,
                ReferenceOr::Reference { reference } => {
                    self.record(
                        Severity::Dropped,
                        path.clone(),
                        "$ref path item",
                        format!(
                            "path item is a $ref to `{reference}`; no operations were generated"
                        ),
                    );
                    continue;
                }
            };

            let ops: [(&str, &Option<Operation>); 5] = [
                ("get", &path_item.get),
                ("put", &path_item.put),
                ("post", &path_item.post),
                ("patch", &path_item.patch),
                ("delete", &path_item.delete),
            ];
            for (method, op) in ops {
                let Some(op) = op else { continue };
                self.diagnose_operation_bodies(op, spec.components.as_ref(), method, path);
                if let Some(entity) = self.parse_query(op, spec.components.as_ref(), method, path) {
                    entities.push(entity);
                }
            }
        }

        // Unions are built optimistically per schema; only now, with the whole
        // entity list in hand, can their members be checked against what was
        // actually generated.
        self.resolve_unions(&mut entities);

        // Only now is the final type list known, so only now can a field's type be
        // checked against it.
        let spec_schema_names: HashSet<String> = spec
            .components
            .as_ref()
            .map(|components| components.schemas.keys().cloned().collect())
            .unwrap_or_default();
        self.diagnose_undefined_field_types(&entities, &spec_schema_names);

        entities
    }

    /// Report fields typed with something that was never generated.
    ///
    /// A `$ref` to a schema that is not in the spec, or to one that produced no
    /// type (an `allOf` schema, a `oneOf` whose members were all dropped), leaves
    /// the field naming a type nothing defines. The generator cannot invent it and
    /// the crate will not compile, so this is fatal. External `$ref`s are excluded:
    /// they are unsupported by design and carry their own diagnostic.
    fn diagnose_undefined_field_types(
        &mut self,
        entities: &[Entity],
        spec_schema_names: &HashSet<String>,
    ) {
        let generated: HashSet<&str> = entities
            .iter()
            .map(|entity| match entity {
                Entity::Struct(s) => s.name.as_str(),
                Entity::Enum(e) => e.name.as_str(),
                Entity::Union(u) => u.name.as_str(),
            })
            .collect();

        for entity in entities {
            let Entity::Struct(s) = entity else { continue };
            for field in &s.fields {
                // An inline enum's type is generated by the writer, so it is not in
                // the entity list.
                if field.is_inline_enum {
                    continue;
                }
                if field
                    .ref_target
                    .as_deref()
                    .is_some_and(|reference| !is_local_schema_ref(reference))
                {
                    continue;
                }
                let ty = element_type(&field.rust_type);
                if BUILTIN_TYPES.contains(&ty) || generated.contains(ty) {
                    continue;
                }

                let reason = match &field.ref_target {
                    Some(reference) => {
                        let target = resolve_ref_name(reference);
                        if spec_schema_names.contains(&target) {
                            format!(
                                "$ref \"{reference}\" names schema `{target}`, which produced no type"
                            )
                        } else {
                            format!("$ref \"{reference}\" names a schema that does not exist")
                        }
                    }
                    None => format!("field type `{ty}` was never generated"),
                };
                self.record(
                    Severity::Fatal,
                    format!("{}.{}", s.name, field.name),
                    "$ref",
                    reason,
                );
            }
        }
    }

    /// Record a diagnostic for a `$ref` that does not point at a local component
    /// schema: the generated type name is the raw ref string and will not compile.
    fn diagnose_ref(&mut self, reference: &str, context: &str) {
        if !is_local_schema_ref(reference) {
            self.record(
                Severity::Degraded,
                context.to_string(),
                "external $ref",
                format!(
                    "$ref `{reference}` is not a local component schema; the generated type name likely will not compile"
                ),
            );
        }
    }

    /// The Rust type a `$ref` names.
    ///
    /// A reference whose target has no Rust name (`#/components/schemas/!!!`) is
    /// fatal, so the type returned here is never emitted; `serde_json::Value` just
    /// keeps the signature honest about returning a real type.
    fn ref_type_name(&mut self, reference: &str, context: &str) -> String {
        let target = resolve_ref_name(reference);
        match to_type_ident(&target) {
            Some(name) => name,
            None => {
                self.record(
                    Severity::Fatal,
                    context.to_string(),
                    "$ref",
                    format!("$ref \"{reference}\" has nothing a Rust name can be built from"),
                );
                "serde_json::Value".to_string()
            }
        }
    }
}

/// Rust types the generator emits without generating a definition for them.
const BUILTIN_TYPES: &[&str] = &[
    "String",
    "i32",
    "i64",
    "f64",
    "bool",
    "DateTime<Utc>",
    "NaiveDate",
    "Uuid",
    "serde_json::Value",
];

/// Strip `Vec<...>` wrappers down to the type a field ultimately names.
fn element_type(rust_type: &str) -> &str {
    let mut ty = rust_type;
    while let Some(inner) = ty.strip_prefix("Vec<").and_then(|t| t.strip_suffix('>')) {
        ty = inner;
    }
    ty
}

/// Extract the schema name from a `$ref` string (e.g. `#/components/schemas/Foo` -> `Foo`).
fn resolve_ref_name(reference: &str) -> String {
    reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or(reference)
        .to_string()
}

/// Whether a `$ref` points at a local component schema (the only kind we can
/// turn into a valid Rust type name).
fn is_local_schema_ref(reference: &str) -> bool {
    reference.starts_with("#/components/schemas/")
}

#[cfg(test)]
mod tests {
    use crate::parse::testutil::*;

    /// A fully-supported spec must produce no diagnostics (guards against noise).
    #[test]
    fn diagnostic_clean_spec_is_silent() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
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
      responses:
        '200':
          description: OK
components:
  schemas:
    Thing:
      type: object
      required: [name]
      properties:
        name:
          type: string
          minLength: 1
        tags:
          type: array
          items:
            type: string
"
        ))?;
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");

        Ok(())
    }
}
