//! OpenAPI spec to [`Entity`], with a [`Diagnostic`] for every construct that
//! is dropped or degraded.
//!
//! [`parse`] walks the spec and the submodules hold one area of the walk each,
//! reaching back for what they share: the `$ref` helpers and [`diagnose_ref`].
//!
//! Every function that can report a loss returns its diagnostics, so a
//! signature says what a function produces and the caller splices the result
//! into its own list. No function writes into a collector it does not own.

mod constraint;
mod default;
mod one_of;
mod operation;
mod schema;
#[cfg(test)]
mod testutil;

use std::collections::{HashMap, HashSet};

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
    let mut entities = Vec::new();
    let mut diagnostics = Vec::new();

    // Rust type name → the spec construct that claimed it. Every generated type
    // is named from a schema or from an operation, so one map catches every
    // pair that would emit two items under one name. See [`claim_type_name`].
    let mut type_names: HashMap<String, String> = HashMap::new();

    if let Some(components) = &spec.components {
        for (name, ref_or) in &components.schemas {
            let schema = match ref_or {
                ReferenceOr::Item(schema) => schema,
                ReferenceOr::Reference { reference } => {
                    record(
                        &mut diagnostics,
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
                record(
                    &mut diagnostics,
                    Severity::Fatal,
                    format!("components.schemas.{name}"),
                    "schema",
                    format!("schema name \"{name}\" has nothing a Rust name can be built from"),
                );
                continue;
            };
            diagnostics.extend(claim_type_name(
                &mut type_names,
                &type_name,
                format!("schema \"{name}\""),
                format!("components.schemas.{name}"),
                "schema",
            ));

            // Each arm returns its own diagnostics, so the one that produces the
            // entity is the only one whose reports are kept.
            let produced = schema::parse_schema(name, schema)
                .or_else(|| one_of::parse_one_of(name, schema))
                .or_else(|| {
                    schema::parse_enum(name, schema, &format!("components.schemas.{name}"))
                        .map(|(enum_def, reported)| (Entity::Enum(enum_def), reported))
                });

            match produced {
                Some((entity, reported)) => {
                    diagnostics.extend(reported);
                    entities.push(entity);
                }
                None => diagnostics.extend(schema::diagnose_unsupported_schema(name, schema)),
            }
        }
    }

    for (path, path_item_ref) in spec.paths.iter() {
        let path_item = match path_item_ref {
            ReferenceOr::Item(item) => item,
            ReferenceOr::Reference { reference } => {
                record(
                    &mut diagnostics,
                    Severity::Dropped,
                    path.clone(),
                    "$ref path item",
                    format!("path item is a $ref to `{reference}`; no operations were generated"),
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
            diagnostics.extend(operation::diagnose_operation_bodies(
                op,
                spec.components.as_ref(),
                method,
                path,
            ));
            let (entity, reported) =
                operation::parse_query(op, spec.components.as_ref(), method, path);
            diagnostics.extend(reported);
            if let Some(entity) = entity {
                // A query struct is named from the `operationId` (or the path),
                // so it clashes with a schema, or with another operation, the
                // same way two schemas clash.
                if let Entity::Struct(s) = &entity {
                    diagnostics.extend(claim_type_name(
                        &mut type_names,
                        &s.name,
                        operation_origin(op, method, path),
                        format!("{} {path}", method.to_uppercase()),
                        "operation",
                    ));
                }
                entities.push(entity);
            }
        }
    }

    // Unions are built optimistically per schema; only now, with the whole
    // entity list in hand, can their members be checked against what was
    // actually generated.
    diagnostics.extend(one_of::resolve_unions(&mut entities));

    // Only now is the final type list known, so only now can a field's type be
    // checked against it.
    let spec_schema_names: HashSet<String> = spec
        .components
        .as_ref()
        .map(|components| components.schemas.keys().cloned().collect())
        .unwrap_or_default();
    diagnostics.extend(diagnose_undefined_field_types(
        &entities,
        &spec_schema_names,
    ));

    (entities, diagnostics)
}

/// Claim a Rust type name for one spec construct, or report the clash.
///
/// Spec names are sanitized into Rust type names, so two constructs can land on
/// the same one: `foo-bar` and `foo_bar` both give `FooBar`, and two operations
/// that share an `operationId` both give one query struct. Neither generating
/// both nor picking one is defensible, since a `$ref` to either is
/// indistinguishable in the output, so the clash is fatal.
///
/// `origin` names the claiming construct (`schema "foo-bar"`) and is what a
/// later clash quotes back, so both sides of the pair are in the message.
fn claim_type_name(
    type_names: &mut HashMap<String, String>,
    type_name: &str,
    origin: String,
    path: String,
    construct: &'static str,
) -> Option<Diagnostic> {
    match type_names.get(type_name) {
        Some(first) => Some(Diagnostic::new(
            Severity::Fatal,
            path,
            construct,
            format!("{first} and {origin} would both become the Rust type `{type_name}`"),
        )),
        None => {
            type_names.insert(type_name.to_string(), origin);
            None
        }
    }
}

/// How a diagnostic names the operation a query struct came from.
fn operation_origin(op: &Operation, method: &str, path: &str) -> String {
    let location = format!("{} {path}", method.to_uppercase());
    match &op.operation_id {
        Some(id) => format!("operation \"{id}\" ({location})"),
        None => format!("operation {location}"),
    }
}

/// Report fields typed with something that was never generated.
///
/// A `$ref` to a schema that is not in the spec, or to one that produced no
/// type (an `allOf` schema, a `oneOf` whose members were all dropped), leaves
/// the field naming a type nothing defines. The generator cannot invent it and
/// the crate will not compile, so this is fatal. External `$ref`s are excluded:
/// they are unsupported by design and carry their own diagnostic.
fn diagnose_undefined_field_types(
    entities: &[Entity],
    spec_schema_names: &HashSet<String>,
) -> Vec<Diagnostic> {
    let generated: HashSet<&str> = entities
        .iter()
        .map(|entity| match entity {
            Entity::Struct(s) => s.name.as_str(),
            Entity::Enum(e) => e.name.as_str(),
            Entity::Union(u) => u.name.as_str(),
        })
        .collect();

    let mut diagnostics = Vec::new();
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
            record(
                &mut diagnostics,
                Severity::Fatal,
                format!("{}.{}", s.name, field.name),
                "$ref",
                reason,
            );
        }
    }
    diagnostics
}

/// Report a `$ref` that does not point at a local component schema: the
/// generated type name is the raw ref string and will not compile.
fn diagnose_ref(reference: &str, context: &str) -> Option<Diagnostic> {
    (!is_local_schema_ref(reference)).then(|| {
        Diagnostic::new(
            Severity::Degraded,
            context,
            "external $ref",
            format!(
                "$ref `{reference}` is not a local component schema; the generated type name likely will not compile"
            ),
        )
    })
}

/// The Rust type a `$ref` names.
///
/// A reference whose target has no Rust name (`#/components/schemas/!!!`) is
/// fatal, so the type returned here is never emitted; `serde_json::Value` just
/// keeps the signature honest about returning a real type.
fn ref_type_name(reference: &str, context: &str) -> (String, Vec<Diagnostic>) {
    let target = resolve_ref_name(reference);
    match to_type_ident(&target) {
        Some(name) => (name, Vec::new()),
        None => (
            "serde_json::Value".to_string(),
            vec![Diagnostic::new(
                Severity::Fatal,
                context,
                "$ref",
                format!("$ref \"{reference}\" has nothing a Rust name can be built from"),
            )],
        ),
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
