use std::fmt::Write;

use crate::{
    Config, Constraints, Diagnostic, Entity, EntityKind, EnumDef, Field, GeneratedCrate,
    GeneratedFile, StructDef, UnionDef,
    diagnostic::{Severity, record},
    ident::{assert_ident, escape_keyword, to_field_ident, to_snake_case},
};

/// Generate a complete crate from a list of parsed entities.
pub fn write(entities: &[Entity], config: &Config) -> Result<GeneratedCrate, std::fmt::Error> {
    let struct_fields = entities.iter().filter_map(|e| match e {
        Entity::Struct(s) => Some(s.fields.as_slice()),
        Entity::Enum(_) | Entity::Union(_) => None,
    });

    let needs_regex = struct_fields.clone().any(|fields| {
        fields.iter().any(|f| {
            matches!(
                &f.constraints,
                Constraints::String {
                    pattern: Some(_),
                    ..
                }
            )
        })
    });

    let needs_uuid = struct_fields
        .clone()
        .any(|fields| fields.iter().any(|f| f.rust_type == "Uuid"));

    let (enum_name_map, _) = resolve_inline_enums(entities);

    // Resolve every field's Rust identifier once: model.rs, default.rs and
    // validation.rs must all spell the same field the same way.
    let (field_idents, diagnostics) = resolve_field_idents(entities);
    // Render every field default once; both model.rs and default.rs consume the
    // result.
    let default_literals = compute_default_literals(entities, &enum_name_map);

    // A spec with no usable default writes no `default.rs`, so the module
    // declaration in lib.rs is conditional too.
    let needs_defaults = !default_literals.is_empty();

    let mut files = vec![
        GeneratedFile {
            path: "Cargo.toml",
            content: generate_cargo_toml(config, needs_regex, needs_uuid),
        },
        GeneratedFile {
            path: "src/lib.rs",
            content: generate_lib_rs(needs_defaults),
        },
        GeneratedFile {
            path: "src/validation.rs",
            content: write_validation_rs(entities, needs_regex, &field_idents)?,
        },
        GeneratedFile {
            path: "src/model.rs",
            content: write_model_rs(entities, &enum_name_map, &default_literals, &field_idents)?,
        },
    ];

    if needs_defaults {
        files.push(GeneratedFile {
            path: "src/default.rs",
            content: write_default_rs(entities, &enum_name_map, &default_literals, &field_idents)?,
        });
    }

    Ok(GeneratedCrate { files, diagnostics })
}

fn header_comment() -> &'static str {
    "This file is @generated. Do not edit manually."
}

/// Collect deduplicated inline enum names from struct entities.
///
/// Returns a map from `(entity_name, raw_enum_name)` → resolved prefixed name,
/// plus a vec of `(resolved_name, &EnumDef)` for the unique enums to emit.
type EnumNameMap = std::collections::HashMap<(String, String), String>;

/// Rust identifier for each field, keyed by `(struct name, spec field name)`.
/// A field is absent only if its struct is, so lookups use [`field_ident`].
type FieldIdents = std::collections::HashMap<(String, String), String>;

/// Resolve the Rust identifier of every field of every struct.
///
/// Property names are arbitrary spec strings, so the identifier can differ from
/// the name on the wire (`first-name` → `first_name`); `write_model_rs` emits a
/// `#[serde(rename)]` whenever it does, which keeps the wire format intact.
/// Sanitizing can map two properties of one struct onto a single identifier
/// (`first-name` and `first.name`), which no naming choice resolves honestly,
/// so that is [`Severity::Fatal`] and [`crate::generate`] fails on it.
fn resolve_field_idents(entities: &[Entity]) -> (FieldIdents, Vec<Diagnostic>) {
    let mut idents = FieldIdents::new();
    let mut diagnostics = Vec::new();
    for entity in entities {
        let Entity::Struct(s) = entity else { continue };
        // Field identifier → the first property that claimed it.
        let mut taken: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for field in &s.fields {
            let Some(ident) = to_field_ident(&field.name) else {
                record(
                    &mut diagnostics,
                    Severity::Fatal,
                    format!("{}.{}", s.name, field.name),
                    "property name",
                    format!(
                        "property name \"{}\" has nothing a Rust name can be built from",
                        field.name
                    ),
                );
                continue;
            };
            if let Some(first) = taken.get(&ident) {
                record(
                    &mut diagnostics,
                    Severity::Fatal,
                    format!("{}.{}", s.name, field.name),
                    "property name",
                    format!(
                        "properties \"{first}\" and \"{}\" would both become \
                         the Rust struct field `{ident}`",
                        field.name
                    ),
                );
            } else {
                taken.insert(ident.clone(), field.name.clone());
            }
            idents.insert((s.name.clone(), field.name.clone()), ident);
        }
    }
    (idents, diagnostics)
}

/// The Rust identifier a field is emitted with, or `None` for a property name
/// that has none. Such a field is already reported as fatal by
/// [`resolve_field_idents`], so the writers skip it: the run produces no files.
fn field_ident(struct_name: &str, field: &Field, field_idents: &FieldIdents) -> Option<String> {
    field_idents
        .get(&(struct_name.to_string(), field.name.clone()))
        .cloned()
        .or_else(|| to_field_ident(&field.name))
}

/// Name of the generated `default.rs` function for a field.
///
/// The name is the struct and the field, with no `default_` prefix: the
/// `default::` module path the caller writes already says what it returns.
///
/// Leading underscores are trimmed from both halves: `series__10min` would
/// contain a double underscore and trip rustc's `non_snake_case` lint. The
/// struct half opens the name, and a type identifier never starts with a
/// digit, so the name stays a valid identifier without the prefix.
fn default_fn_name(struct_name: &str, field_ident: &str) -> String {
    let name = format!(
        "{}_{}",
        to_snake_case(struct_name).trim_start_matches('_'),
        field_ident.trim_start_matches('_')
    );
    assert_ident(&name).to_string()
}

/// Escape a spec string for use inside a Rust string literal.
fn escape_literal(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Rendered default-value literals keyed by `(struct name, field name)`.
/// A missing key means the field has no default.
type DefaultLiterals = std::collections::HashMap<(String, String), String>;

/// Render the Rust literal for every field default up front, the single source
/// for both `model.rs` (whether to emit `#[serde(default)]`) and `default.rs`
/// (the function body).
///
/// Every default reaching here was passed by `parse::classify_default`, which
/// only keeps what this can render. A failure is therefore a generator bug, and
/// panicking says so: the alternative is a field that silently loses its
/// `#[serde(default)]` and becomes required on the wire.
fn compute_default_literals(entities: &[Entity], enum_name_map: &EnumNameMap) -> DefaultLiterals {
    let mut literals = DefaultLiterals::new();
    for entity in entities {
        let Entity::Struct(s) = entity else { continue };
        for field in &s.fields {
            let Some(default_val) = &field.default_value else {
                continue;
            };
            // The enum the default has to name a variant of, if this is an
            // inline enum field. `rust_type` is the enum's unprefixed name.
            let enum_def = s.enums.iter().find(|e| e.name == field.rust_type);
            let literal = format_default_literal(
                default_val,
                field,
                &resolved_type(&s.name, field, enum_name_map),
                enum_def,
            )
            .unwrap_or_else(|| {
                panic!(
                    "generator bug: default {default_val} for `{}.{}` passed parse::classify_default \
                     but has no Rust literal for type `{}`",
                    s.name, field.name, field.rust_type
                )
            });
            literals.insert((s.name.clone(), field.name.clone()), literal);
        }
    }
    literals
}

/// The Rust type name a field is emitted with, before `Option` wrapping.
///
/// Only an inline enum needs resolving: its `rust_type` is the unprefixed enum
/// name, which [`resolve_inline_enums`] maps to the deduplicated, struct-
/// prefixed name. Every other `rust_type` is already a real type name and is
/// returned untouched: looking it up in the map would rewrite a `$ref` field
/// whose target schema happens to share an inline enum's name.
fn resolved_type(struct_name: &str, field: &Field, enum_name_map: &EnumNameMap) -> String {
    if !field.is_inline_enum {
        return field.rust_type.clone();
    }
    enum_name_map
        .get(&(struct_name.to_string(), field.rust_type.clone()))
        .cloned()
        .unwrap_or_else(|| field.rust_type.clone())
}

fn resolve_inline_enums(entities: &[Entity]) -> (EnumNameMap, Vec<(String, &EnumDef)>) {
    let mut enum_name_map: EnumNameMap = EnumNameMap::new();
    let mut final_enums: Vec<(String, &EnumDef)> = Vec::new();
    let mut variants_to_name: std::collections::HashMap<Vec<String>, String> =
        std::collections::HashMap::new();

    for entity in entities {
        if let Entity::Struct(s) = entity {
            for enum_def in &s.enums {
                let variants: Vec<String> = enum_def.variants.iter().map(|p| p.1.clone()).collect();

                if let Some(existing_name) = variants_to_name.get(&variants) {
                    enum_name_map.insert(
                        (s.name.clone(), enum_def.name.clone()),
                        existing_name.clone(),
                    );
                } else {
                    let prefixed = format!("{}{}", s.name, enum_def.name);
                    enum_name_map.insert((s.name.clone(), enum_def.name.clone()), prefixed.clone());
                    variants_to_name.insert(variants, prefixed.clone());
                    final_enums.push((prefixed, enum_def));
                }
            }
        }
    }

    (enum_name_map, final_enums)
}

fn write_enum(out: &mut String, name: &str, enum_def: &EnumDef) -> std::fmt::Result {
    writeln!(out)?;
    writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]")?;
    writeln!(out, "pub enum {} {{", assert_ident(name))?;
    for (variant, original) in &enum_def.variants {
        // The variant name is sanitized (`10min` → `_10min`); the rename is what
        // keeps the spec's value on the wire.
        writeln!(
            out,
            "    #[serde(rename = \"{}\")]",
            escape_literal(original)
        )?;
        writeln!(out, "    {},", assert_ident(variant))?;
    }
    writeln!(out, "}}")
}

/// Emit a union enum from a top-level `oneOf` schema. Internally tagged when
/// `tag` is set (a discriminator was present), untagged otherwise.
fn write_union(out: &mut String, union_def: &UnionDef) -> std::fmt::Result {
    writeln!(out)?;
    writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]")?;
    match &union_def.tag {
        Some(prop) => writeln!(out, "#[serde(tag = \"{prop}\")]")?,
        None => writeln!(out, "#[serde(untagged)]")?,
    }
    writeln!(out, "pub enum {} {{", assert_ident(&union_def.name))?;
    for variant in &union_def.variants {
        if let Some(wire) = &variant.wire_value
            && *wire != variant.variant_name
        {
            writeln!(out, "    #[serde(rename = \"{}\")]", escape_literal(wire))?;
        }
        writeln!(
            out,
            "    {}({}),",
            assert_ident(&variant.variant_name),
            assert_ident(&variant.inner_type)
        )?;
    }
    writeln!(out, "}}")
}

fn write_model_rs(
    entities: &[Entity],
    enum_name_map: &EnumNameMap,
    default_literals: &DefaultLiterals,
    field_idents: &FieldIdents,
) -> Result<String, std::fmt::Error> {
    let struct_fields = entities.iter().filter_map(|e| match e {
        Entity::Struct(s) => Some(&s.fields),
        Entity::Enum(_) | Entity::Union(_) => None,
    });
    let has_type = |ty: &str| {
        struct_fields
            .clone()
            .any(|fields| fields.iter().any(|f| f.rust_type.contains(ty)))
    };

    let needs_datetime = has_type("DateTime");
    let needs_naive_date = has_type("NaiveDate");
    let needs_uuid = has_type("Uuid");

    let chrono_import = match (needs_datetime, needs_naive_date) {
        (true, true) => "\nuse chrono::{DateTime, NaiveDate, Utc};",
        (true, false) => "\nuse chrono::{DateTime, Utc};",
        (false, true) => "\nuse chrono::NaiveDate;",
        (false, false) => "",
    };
    let uuid_import = if needs_uuid { "\nuse uuid::Uuid;" } else { "" };

    let (_, final_enums) = resolve_inline_enums(entities);

    let header = header_comment();
    let mut out = format!(
        "\
// {header}
{chrono_import}
use serde::{{Deserialize, Serialize}};{uuid_import}
"
    );

    // Emit enums (standalone + inline)
    for entity in entities {
        if let Entity::Enum(enum_def) = entity {
            write_enum(&mut out, &enum_def.name, enum_def)?;
        }
    }
    for (enum_name, enum_def) in &final_enums {
        write_enum(&mut out, enum_name, enum_def)?;
    }

    // Emit unions (from top-level oneOf schemas)
    for entity in entities {
        if let Entity::Union(union_def) = entity {
            write_union(&mut out, union_def)?;
        }
    }

    for entity in entities {
        let Entity::Struct(s) = entity else {
            continue;
        };
        writeln!(out)?;
        match s.kind {
            EntityKind::Schema => {
                writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]")?;
            }
            EntityKind::Query => {
                writeln!(out, "#[derive(Debug, Clone, Deserialize)]")?;
            }
        }
        // A field-less struct is reachable via an empty `properties`, or via a
        // tagged union absorbing its only property into the tag.
        if s.fields.is_empty() {
            writeln!(out, "pub struct {} {{}}", assert_ident(&s.name))?;
            continue;
        }
        writeln!(out, "pub struct {} {{", assert_ident(&s.name))?;
        for field in &s.fields {
            let resolved = resolved_type(&s.name, field, enum_name_map);
            let final_type = if field.is_optional {
                format!("Option<{resolved}>")
            } else {
                resolved
            };
            let Some(ident) = field_ident(&s.name, field, field_idents) else {
                continue;
            };
            // A property name that is not a usable Rust identifier is renamed;
            // the rename keeps the spec's name on the wire. A raw identifier
            // (`r#type`) needs none: serde already strips the `r#`.
            if ident != field.name {
                writeln!(
                    out,
                    "    #[serde(rename = \"{}\")]",
                    escape_literal(&field.name)
                )?;
            }
            // Emit the serde attribute only when a literal was rendered (so it
            // never references a `default.rs` function that was skipped).
            if default_literals.contains_key(&(s.name.clone(), field.name.clone())) {
                let fn_name = default_fn_name(&s.name, &ident);
                writeln!(out, "    #[serde(default = \"crate::default::{fn_name}\")]")?;
            }
            writeln!(
                out,
                "    pub {}: {final_type},",
                escape_keyword(assert_ident(&ident))
            )?;
        }
        writeln!(out, "}}")?;
    }

    Ok(out)
}

/// Convert a JSON default value to a Rust literal string for the given field.
/// `resolved` is the field's emitted type name (see [`resolved_type`]).
/// Returns `None` if the combination is unsupported.
fn format_default_literal(
    value: &serde_json::Value,
    field: &Field,
    resolved: &str,
    enum_def: Option<&EnumDef>,
) -> Option<String> {
    let literal = match (value, field.rust_type.as_str()) {
        (serde_json::Value::String(s), "String") => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!("String::from(\"{escaped}\")")
        }
        (serde_json::Value::Number(n), "i32" | "i64") => {
            format!("{}", n.as_i64()?)
        }
        (serde_json::Value::Number(n), "f64") => {
            let f = n.as_f64()?;
            if f.fract() == 0.0 {
                format!("{f:.1}_f64")
            } else {
                format!("{f}_f64")
            }
        }
        (serde_json::Value::Bool(b), "bool") => format!("{b}"),
        (serde_json::Value::String(s), "DateTime<Utc>") => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!(
                "\"{escaped}\".parse::<DateTime<Utc>>().expect(\"hardcoded default from OpenAPI spec\")"
            )
        }
        (serde_json::Value::String(s), "NaiveDate") => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!(
                "\"{escaped}\".parse::<NaiveDate>().expect(\"hardcoded default from OpenAPI spec\")"
            )
        }
        (serde_json::Value::String(s), "Uuid") => {
            let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
            format!(
                "Uuid::parse_str(\"{escaped}\").expect(\"hardcoded default from OpenAPI spec\")"
            )
        }
        // Inline enum: the variant name is looked up rather than re-derived, so
        // a default can never name a variant the enum does not have. `parse`
        // has already rejected a default that is not one of the enum's values.
        (serde_json::Value::String(s), _) if field.is_inline_enum => {
            let variant = enum_def?
                .variants
                .iter()
                .find_map(|(variant, original)| (original == s).then_some(variant))?;
            format!("{resolved}::{variant}")
        }
        _ => return None,
    };
    if field.is_optional {
        Some(format!("Some({literal})"))
    } else {
        Some(literal)
    }
}

fn write_default_rs(
    entities: &[Entity],
    enum_name_map: &EnumNameMap,
    default_literals: &DefaultLiterals,
    field_idents: &FieldIdents,
) -> Result<String, std::fmt::Error> {
    let struct_fields_with_name = entities.iter().filter_map(|e| match e {
        Entity::Struct(s) => Some((&s.name, &s.fields)),
        Entity::Enum(_) | Entity::Union(_) => None,
    });

    // Determine which chrono/uuid imports are needed in default functions
    let needs_datetime = struct_fields_with_name.clone().any(|(_, fields)| {
        fields
            .iter()
            .any(|f| f.default_value.is_some() && f.rust_type.contains("DateTime"))
    });
    let needs_naive_date = struct_fields_with_name.clone().any(|(_, fields)| {
        fields
            .iter()
            .any(|f| f.default_value.is_some() && f.rust_type.contains("NaiveDate"))
    });
    let needs_uuid = struct_fields_with_name.clone().any(|(_, fields)| {
        fields
            .iter()
            .any(|f| f.default_value.is_some() && f.rust_type == "Uuid")
    });

    let chrono_import = match (needs_datetime, needs_naive_date) {
        (true, true) => "use chrono::{DateTime, NaiveDate, Utc};\n",
        (true, false) => "use chrono::{DateTime, Utc};\n",
        (false, true) => "use chrono::NaiveDate;\n",
        (false, false) => "",
    };
    let uuid_import = if needs_uuid { "use uuid::Uuid;\n" } else { "" };

    // Check if any default references an enum type from model
    let needs_model_import = struct_fields_with_name.clone().any(|(_, fields)| {
        fields
            .iter()
            .any(|f| f.default_value.is_some() && f.is_inline_enum)
    });
    let model_import = if needs_model_import {
        "use crate::model::*;\n"
    } else {
        ""
    };

    let header = header_comment();
    // Import order matches rustfmt's alphabetical sort: chrono < crate < uuid.
    let mut out = format!(
        "\
// {header}

{chrono_import}{model_import}{uuid_import}"
    );

    for entity in entities {
        let Entity::Struct(s) = entity else {
            continue;
        };
        for field in &s.fields {
            // Skip fields with no default, and those whose default could not be
            // rendered (absent from the map, already reported in `write`).
            let Some(literal) = default_literals.get(&(s.name.clone(), field.name.clone())) else {
                continue;
            };
            let resolved = resolved_type(&s.name, field, enum_name_map);
            let return_type = if field.is_optional {
                format!("Option<{resolved}>")
            } else {
                resolved
            };
            let Some(ident) = field_ident(&s.name, field, field_idents) else {
                continue;
            };
            let fn_name = default_fn_name(&s.name, &ident);
            writeln!(out)?;
            writeln!(out, "pub fn {fn_name}() -> {return_type} {{")?;
            writeln!(out, "    {literal}")?;
            writeln!(out, "}}")?;
        }
    }

    Ok(out)
}

fn write_validation_rs(
    entities: &[Entity],
    needs_regex: bool,
    field_idents: &FieldIdents,
) -> Result<String, std::fmt::Error> {
    // Import order matches rustfmt's alphabetical sort: crate < regex.
    let imports = if needs_regex {
        "use crate::model::*;\nuse regex::Regex;\n"
    } else {
        "use crate::model::*;\n"
    };
    let header = header_comment();
    let mut out = format!(
        "\
// {header}

{imports}
#[derive(Debug)]
pub struct ValidationError {{
    pub details: Vec<String>,
}}

impl std::fmt::Display for ValidationError {{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{
        write!(f, \"validation failed: {{}}\", self.details.join(\"; \"))
    }}
}}

impl std::error::Error for ValidationError {{}}

pub trait Validation {{
    fn validate(&self) -> Result<(), ValidationError> {{
        Ok(())
    }}
}}

impl<T: Validation> Validation for Vec<T> {{
    fn validate(&self) -> Result<(), ValidationError> {{
        let mut errors = Vec::new();
        for (i, item) in self.iter().enumerate() {{
            if let Err(e) = item.validate() {{
                for detail in e.details {{
                    errors.push(format!(\"[{{i}}]: {{detail}}\"));
                }}
            }}
        }}
        if errors.is_empty() {{
            Ok(())
        }} else {{
            Err(ValidationError {{ details: errors }})
        }}
    }}
}}
"
    );

    let (_, inline_enums) = resolve_inline_enums(entities);

    for entity in entities {
        match entity {
            Entity::Struct(s) => write_validation_impl(&mut out, s, field_idents)?,
            Entity::Enum(e) => {
                writeln!(out)?;
                writeln!(out, "impl Validation for {} {{}}", assert_ident(&e.name))?;
            }
            Entity::Union(u) => write_union_validation_impl(&mut out, u)?,
        }
    }

    for (enum_name, _) in &inline_enums {
        writeln!(out)?;
        writeln!(out, "impl Validation for {} {{}}", assert_ident(enum_name))?;
    }

    Ok(out)
}

/// Write the `impl Validation` block for a struct entity.
fn write_validation_impl(
    out: &mut String,
    s: &StructDef,
    field_idents: &FieldIdents,
) -> std::fmt::Result {
    let name = assert_ident(&s.name);
    let fields = &s.fields;
    let has_any_checks = fields.iter().any(|f| f.constraints.has_checks());

    writeln!(out)?;

    if !has_any_checks {
        writeln!(out, "impl Validation for {name} {{}}")?;
        return Ok(());
    }

    writeln!(out, "impl Validation for {name} {{")?;

    writeln!(
        out,
        "    fn validate(&self) -> Result<(), ValidationError> {{"
    )?;
    writeln!(out, "        let mut errors = Vec::new();")?;

    for field in fields {
        if !field.constraints.has_checks() {
            continue;
        }
        let Some(ident) = field_ident(&s.name, field, field_idents) else {
            continue;
        };
        write_field_checks(out, field, &ident)?;
    }

    writeln!(out, "        if errors.is_empty() {{")?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(ValidationError {{ details: errors }})"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    Ok(())
}

/// Write the `impl Validation` block for a union entity, delegating to the
/// active variant's inner `validate()`.
fn write_union_validation_impl(out: &mut String, union_def: &UnionDef) -> std::fmt::Result {
    writeln!(out)?;
    if union_def.variants.is_empty() {
        writeln!(
            out,
            "impl Validation for {} {{}}",
            assert_ident(&union_def.name)
        )?;
        return Ok(());
    }
    writeln!(
        out,
        "impl Validation for {} {{",
        assert_ident(&union_def.name)
    )?;
    writeln!(
        out,
        "    fn validate(&self) -> Result<(), ValidationError> {{"
    )?;
    writeln!(out, "        match self {{")?;
    for variant in &union_def.variants {
        writeln!(
            out,
            "            Self::{}(inner) => inner.validate(),",
            assert_ident(&variant.variant_name)
        )?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")
}

/// Write `errors.push(format!("...", args));` across multiple lines for readability.
fn write_error_push(out: &mut String, indent: &str, fmt_str: &str, args: &str) -> std::fmt::Result {
    writeln!(out, "{indent}    errors.push(format!(")?;
    writeln!(out, "{indent}        {fmt_str},")?;
    writeln!(out, "{indent}        {args}")?;
    writeln!(out, "{indent}    ));")
}

/// Returns true when this constraint produces exactly one `if`-style check that
/// clippy would flag as collapsible with an outer `if let Some`.
fn is_single_collapsible_check(constraints: &Constraints) -> bool {
    // VecNested emits a `for` loop, Array unique_items emits a block; neither is collapsible.
    let non_collapsible = matches!(constraints, Constraints::VecNested)
        || matches!(
            constraints,
            Constraints::Array {
                unique_items: true,
                min_items: None,
                max_items: None,
                ..
            }
        );
    !non_collapsible && constraints.n_checks() == 1
}

/// Open a check: either a plain `if condition {` or a collapsed
/// `if let Some(val) = &self.field\n    && condition\n{`.
fn write_check_open(
    out: &mut String,
    indent: &str,
    condition: &str,
    guard: Option<&str>,
) -> std::fmt::Result {
    if let Some(guard) = guard {
        writeln!(out, "{indent}{guard}")?;
        writeln!(out, "{indent}    && {condition}")?;
        writeln!(out, "{indent}{{")
    } else {
        writeln!(out, "{indent}if {condition} {{")
    }
}

/// Write the validation checks for a single field inside a `validate()` body.
fn write_field_checks(out: &mut String, field: &Field, ident: &str) -> std::fmt::Result {
    // The spec's property name is what error messages report; `ident` is how the
    // field is spelled in Rust, and the two differ for a sanitized name.
    let name = &field.name;
    let field_ident = escape_keyword(assert_ident(ident));

    let collapsed = field.is_optional && is_single_collapsible_check(&field.constraints);

    // For optional fields, wrap in `if let Some` (unless collapsed into a single check)
    let (accessor, indent) = if field.is_optional {
        if !collapsed {
            writeln!(out, "        if let Some(val) = &self.{field_ident} {{")?;
        }
        (
            "(*val)".to_string(),
            if collapsed {
                "        "
            } else {
                "            "
            },
        )
    } else {
        (format!("self.{field_ident}"), "        ")
    };

    let guard = if collapsed {
        Some(format!("if let Some(val) = &self.{field_ident}"))
    } else {
        None
    };

    match &field.constraints {
        Constraints::String {
            min_length,
            max_length,
            pattern,
            enumeration,
        } => {
            if let Some(min) = min_length {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor}.chars().count() < {min}"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: length {{}} is less than minimum {min}\""),
                    &format!("{accessor}.chars().count()"),
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if let Some(max) = max_length {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor}.chars().count() > {max}"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: length {{}} exceeds maximum {max}\""),
                    &format!("{accessor}.chars().count()"),
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if let Some(pat) = pattern {
                let escaped = pat.replace('\\', "\\\\").replace('"', "\\\"");
                write_check_open(
                    out,
                    indent,
                    &format!("!Regex::new(\"{escaped}\").unwrap().is_match(&{accessor})"),
                    guard.as_deref(),
                )?;
                // The message is a `format!` string literal, so any `{`/`}` in the
                // pattern (e.g. brace quantifiers like `{1,14}`) must be doubled to
                // avoid being parsed as format placeholders. The `Regex::new` arg
                // above must keep the un-doubled braces.
                let escaped_msg = escaped.replace('{', "{{").replace('}', "}}");
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value '{{}}' does not match pattern '{escaped_msg}'\""),
                    &accessor,
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if !enumeration.is_empty() {
                let values: Vec<String> = enumeration.iter().map(|v| format!("\"{v}\"")).collect();
                let joined = values.join(", ");
                let allowed_display = enumeration.join(", ");
                write_check_open(
                    out,
                    indent,
                    &format!("![{joined}].contains(&{accessor}.as_str())"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value '{{}}' is not one of [{allowed_display}]\""),
                    &accessor,
                )?;
                writeln!(out, "{indent}}}")?;
            }
        }
        Constraints::Integer {
            minimum,
            maximum,
            exclusive_minimum,
            exclusive_maximum,
            multiple_of,
            enumeration,
        } => {
            if let Some(min) = minimum {
                if *exclusive_minimum {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} <= {min}"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is not greater than {min}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                } else {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} < {min}"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is less than minimum {min}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                }
            }
            if let Some(max) = maximum {
                if *exclusive_maximum {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} >= {max}"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is not less than {max}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                } else {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} > {max}"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} exceeds maximum {max}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                }
            }
            if let Some(mult) = multiple_of {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor} % {mult} != 0"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value {{}} is not a multiple of {mult}\""),
                    &accessor,
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if !enumeration.is_empty() {
                let values: Vec<String> = enumeration.iter().map(|v| format!("{v}")).collect();
                let joined = values.join(", ");
                write_check_open(
                    out,
                    indent,
                    &format!("![{joined}].contains(&{accessor})"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value {{}} is not one of [{joined}]\""),
                    &accessor,
                )?;
                writeln!(out, "{indent}}}")?;
            }
        }
        Constraints::Number {
            minimum,
            maximum,
            exclusive_minimum,
            exclusive_maximum,
            multiple_of,
        } => {
            if let Some(min) = minimum {
                if *exclusive_minimum {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} <= {min}_f64"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is not greater than {min}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                } else {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} < {min}_f64"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is less than minimum {min}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                }
            }
            if let Some(max) = maximum {
                if *exclusive_maximum {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} >= {max}_f64"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} is not less than {max}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                } else {
                    write_check_open(
                        out,
                        indent,
                        &format!("{accessor} > {max}_f64"),
                        guard.as_deref(),
                    )?;
                    write_error_push(
                        out,
                        indent,
                        &format!("\"{name}: value {{}} exceeds maximum {max}\""),
                        &accessor,
                    )?;
                    writeln!(out, "{indent}}}")?;
                }
            }
            if let Some(mult) = multiple_of {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor} % {mult}_f64 != 0.0"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value {{}} is not a multiple of {mult}\""),
                    &accessor,
                )?;
                writeln!(out, "{indent}}}")?;
            }
        }
        Constraints::Array {
            min_items,
            max_items,
            unique_items,
        } => {
            if let Some(min) = min_items {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor}.len() < {min}"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: array length {{}} is less than minimum {min}\""),
                    &format!("{accessor}.len()"),
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if let Some(max) = max_items {
                write_check_open(
                    out,
                    indent,
                    &format!("{accessor}.len() > {max}"),
                    guard.as_deref(),
                )?;
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: array length {{}} exceeds maximum {max}\""),
                    &format!("{accessor}.len()"),
                )?;
                writeln!(out, "{indent}}}")?;
            }
            if *unique_items {
                writeln!(out, "{indent}{{")?;
                writeln!(
                    out,
                    "{indent}    let mut seen = std::collections::HashSet::new();"
                )?;
                writeln!(out, "{indent}    for item in {accessor}.iter() {{")?;
                writeln!(out, "{indent}        if !seen.insert(item) {{")?;
                writeln!(
                    out,
                    "{indent}            errors.push(\"{name}: array contains duplicate items\".to_string());"
                )?;
                writeln!(out, "{indent}            break;")?;
                writeln!(out, "{indent}        }}")?;
                writeln!(out, "{indent}    }}")?;
                writeln!(out, "{indent}}}")?;
            }
        }
        Constraints::Nested => {
            write_check_open(
                out,
                indent,
                &format!("let Err(nested) = {accessor}.validate()"),
                guard.as_deref(),
            )?;
            writeln!(
                out,
                "{indent}    errors.extend(nested.details.into_iter().map(|e| format!(\"{name}.{{e}}\")));"
            )?;
            writeln!(out, "{indent}}}")?;
        }
        Constraints::VecNested => {
            writeln!(
                out,
                "{indent}for (i, item) in {accessor}.iter().enumerate() {{"
            )?;
            writeln!(out, "{indent}    if let Err(nested) = item.validate() {{")?;
            writeln!(
                out,
                "{indent}        errors.extend(nested.details.into_iter().map(|e| format!(\"{name}[{{i}}].{{e}}\")));"
            )?;
            writeln!(out, "{indent}    }}")?;
            writeln!(out, "{indent}}}")?;
        }
        Constraints::None => {}
    }

    if field.is_optional && !collapsed {
        writeln!(out, "        }}")?;
    }

    Ok(())
}

fn generate_cargo_toml(config: &Config, needs_regex: bool, needs_uuid: bool) -> String {
    let header = header_comment();
    if config.use_workspace {
        let regex_dep = if needs_regex {
            "regex.workspace = true\n"
        } else {
            ""
        };
        let uuid_dep = if needs_uuid {
            "uuid = { workspace = true, features = [\"serde\"] }\n"
        } else {
            ""
        };
        format!(
            "\
# {header}

[package]
name = \"{crate_name}\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
chrono = {{ workspace = true, features = [\"serde\"] }}
{regex_dep}serde = {{ workspace = true, features = [\"derive\"] }}
serde_json.workspace = true
{uuid_dep}
[dev-dependencies]
pretty_assertions.workspace = true
",
            crate_name = config.crate_name,
        )
    } else {
        let regex_dep = if needs_regex { "regex = \"1\"\n" } else { "" };
        let uuid_dep = if needs_uuid {
            "uuid = { version = \"1\", features = [\"serde\"] }\n"
        } else {
            ""
        };
        format!(
            "\
# {header}

[package]
name = \"{crate_name}\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
chrono = {{ version = \"0.4\", features = [\"serde\"] }}
{regex_dep}serde = {{ version = \"1\", features = [\"derive\"] }}
serde_json = \"1\"
{uuid_dep}
[dev-dependencies]
pretty_assertions = \"1\"
",
            crate_name = config.crate_name,
        )
    }
}

fn generate_lib_rs(needs_defaults: bool) -> String {
    let header = header_comment();
    let default_mod = if needs_defaults {
        "pub mod default;\n"
    } else {
        ""
    };
    format!(
        "\
// {header}

{default_mod}mod model;
mod validation;

pub use model::*;
pub use validation::{{Validation, ValidationError}};
"
    )
}

/// Escape a name with `r#` if it is a Rust keyword.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Constraints, EntityKind, Field, StructDef};

    type Result = std::result::Result<(), Box<dyn std::error::Error>>;

    fn test_config() -> Config {
        Config {
            crate_name: "test_api".to_string(),
            use_workspace: true,
        }
    }

    fn find_file<'a>(krate: &'a GeneratedCrate, path: &str) -> &'a str {
        &krate
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("missing file: {path}"))
            .content
    }

    fn single_field_entity(name: &str, kind: EntityKind, field: Field) -> Entity {
        Entity::Struct(StructDef {
            name: name.into(),
            kind,
            fields: vec![field],
            enums: vec![],
        })
    }

    fn required_field(name: &str, rust_type: &str, constraints: Constraints) -> Field {
        Field {
            name: name.into(),
            rust_type: rust_type.into(),
            is_optional: false,
            constraints,
            default_value: None,
            ref_target: None,
            is_inline_enum: false,
        }
    }

    #[test]
    fn write_string_pattern_regex() -> Result {
        let entity = single_field_entity(
            "Foo",
            EntityKind::Schema,
            required_field(
                "code",
                "String",
                Constraints::String {
                    min_length: None,
                    max_length: None,
                    pattern: Some("^[A-Z]{3}$".into()),
                    enumeration: vec![],
                },
            ),
        );
        let krate = write(&[entity], &test_config())?;
        let v = find_file(&krate, "src/validation.rs");
        assert!(v.contains("Regex::new("), "missing Regex::new: {v}");
        // Imports are emitted in the order rustfmt sorts them, so running
        // rustfmt over the output does not reshuffle them.
        assert!(
            v.contains("use crate::model::*;\nuse regex::Regex;\n\n#[derive(Debug)]"),
            "imports should be sorted with exactly one blank line after: {v}"
        );
        let cargo = find_file(&krate, "Cargo.toml");
        assert!(
            cargo.contains("regex.workspace = true"),
            "missing regex dep: {cargo}"
        );

        Ok(())
    }

    /// A `pattern` containing brace quantifiers (e.g. `{1,14}`) must have its
    /// braces doubled in the `format!` error-message literal, but NOT in the
    /// `Regex::new(...)` argument, where doubling would corrupt the regex.
    #[test]
    fn write_pattern_with_brace_quantifier() -> Result {
        let entity = single_field_entity(
            "Foo",
            EntityKind::Schema,
            required_field(
                "phone",
                "String",
                Constraints::String {
                    min_length: None,
                    max_length: None,
                    pattern: Some(r"^\+[1-9]\d{1,14}$".into()),
                    enumeration: vec![],
                },
            ),
        );
        let krate = write(&[entity], &test_config())?;
        let v = find_file(&krate, "src/validation.rs");

        // Regex::new argument keeps the un-doubled braces (a valid regex).
        assert!(
            v.contains(r#"Regex::new("^\\+[1-9]\\d{1,14}$")"#),
            "regex arg should keep un-doubled braces: {v}"
        );
        // The error-message format literal doubles the braces.
        assert!(
            v.contains(r#"does not match pattern '^\\+[1-9]\\d{{1,14}}$'"#),
            "message literal should double the braces: {v}"
        );

        Ok(())
    }

    #[test]
    fn write_optional_field_wrapping() -> Result {
        let entity = single_field_entity(
            "Foo",
            EntityKind::Schema,
            Field {
                name: "nickname".into(),
                rust_type: "String".into(),
                is_optional: true,
                constraints: Constraints::String {
                    min_length: Some(1),
                    max_length: None,
                    pattern: None,
                    enumeration: vec![],
                },
                default_value: None,
                ref_target: None,
                is_inline_enum: false,
            },
        );
        let krate = write(&[entity], &test_config())?;
        let v = find_file(&krate, "src/validation.rs");
        assert!(
            v.contains("if let Some(val) = &self.nickname"),
            "missing Option unwrap: {v}"
        );
        let model = find_file(&krate, "src/model.rs");
        assert!(
            model.contains("pub nickname: Option<String>"),
            "field should be Option: {model}"
        );

        Ok(())
    }
}
