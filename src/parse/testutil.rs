//! Spec fixtures and lookup helpers shared by the parser's test modules.

use openapiv3::OpenAPI;

use crate::{Diagnostic, Entity, Field, StructDef, UnionDef, load_spec, parse};

pub(super) type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub(super) const MINIMAL_HEADER: &str = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
"#;

pub(super) fn spec_with_schema(schema_yaml: &str) -> Result<OpenAPI> {
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

pub(super) fn first_struct_fields(entities: &[Entity]) -> &[Field] {
    match &entities[0] {
        Entity::Struct(s) => &s.fields,
        _ => panic!("expected Entity::Struct"),
    }
}

/// Parse a full spec and return only the diagnostics.
pub(super) fn diagnostics_for(yaml: &str) -> Result<Vec<Diagnostic>> {
    let spec = load_spec(yaml)?;
    let (_, diagnostics) = parse(&spec);
    Ok(diagnostics)
}

pub(super) fn find_diag<'a>(diags: &'a [Diagnostic], construct: &str) -> &'a Diagnostic {
    diags
        .iter()
        .find(|d| d.construct == construct)
        .unwrap_or_else(|| panic!("expected a `{construct}` diagnostic in {diags:?}"))
}

pub(super) fn has_construct(diags: &[Diagnostic], construct: &str) -> bool {
    diags.iter().any(|d| d.construct == construct)
}

pub(super) fn count_construct(diags: &[Diagnostic], construct: &str) -> usize {
    diags.iter().filter(|d| d.construct == construct).count()
}

pub(super) fn find_struct<'a>(entities: &'a [Entity], name: &str) -> &'a StructDef {
    entities
        .iter()
        .find_map(|e| match e {
            Entity::Struct(s) if s.name == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected Entity::Struct named {name}"))
}

pub(super) fn field_names(s: &StructDef) -> Vec<&str> {
    s.fields.iter().map(|f| f.name.as_str()).collect()
}

pub(super) fn find_union<'a>(entities: &'a [Entity], name: &str) -> &'a UnionDef {
    entities
        .iter()
        .find_map(|e| match e {
            Entity::Union(u) if u.name == name => Some(u),
            _ => None,
        })
        .unwrap_or_else(|| panic!("expected Entity::Union named {name}"))
}

/// Helper: build a spec with `Cat`/`Dog` object schemas plus a caller-supplied
/// composite schema, and return the loaded `OpenAPI`.
pub(super) fn spec_with_composite_spec(composite_yaml: &str) -> Result<OpenAPI> {
    let composite = composite_yaml
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("    {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let full = format!(
        "{MINIMAL_HEADER}paths: {{}}\ncomponents:\n  schemas:\n    Cat:\n      type: object\n      properties:\n        name:\n          type: string\n    Dog:\n      type: object\n      properties:\n        name:\n          type: string\n{composite}\n"
    );
    Ok(load_spec(&full)?)
}

/// Same as [`spec_with_composite_spec`] but returns the parsed entities.
pub(super) fn spec_with_composite(composite_yaml: &str) -> Result<Vec<Entity>> {
    let (entities, _) = parse(&spec_with_composite_spec(composite_yaml)?);
    Ok(entities)
}
