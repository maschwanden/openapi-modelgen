use std::fmt::Write;

use crate::{
    Config, Constraints, Diagnostic, Entity, EntityKind, EnumDef, Field, GeneratedCrate,
    GeneratedFile, UnionDef,
    diagnostics::{Severity, record},
    parse::to_snake_case,
};

/// Rust strict keywords that must be escaped with `r#` when used as identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

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

    let needs_defaults = struct_fields
        .clone()
        .any(|fields| fields.iter().any(|f| f.default_value.is_some()));

    let (enum_name_map, _) = resolve_inline_enums(entities);

    let mut diagnostics = Vec::new();

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
            content: write_validation_rs(entities, needs_regex)?,
        },
        GeneratedFile {
            path: "src/model.rs",
            content: write_model_rs(entities, &enum_name_map, &mut diagnostics)?,
        },
    ];

    if needs_defaults {
        files.push(GeneratedFile {
            path: "src/default.rs",
            content: write_default_rs(entities, &enum_name_map)?,
        });
    }

    Ok(GeneratedCrate { files, diagnostics })
}

fn header_comment() -> &'static str {
    "This file is @generated — do not edit manually."
}

/// Collect deduplicated inline enum names from struct entities.
///
/// Returns a map from `(entity_name, raw_enum_name)` → resolved prefixed name,
/// plus a vec of `(resolved_name, &EnumDef)` for the unique enums to emit.
type EnumNameMap = std::collections::HashMap<(String, String), String>;

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
    writeln!(out, "pub enum {name} {{")?;
    for (variant, original) in &enum_def.variants {
        writeln!(out, "    #[serde(rename = \"{original}\")]")?;
        writeln!(out, "    {variant},")?;
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
    writeln!(out, "pub enum {} {{", union_def.name)?;
    for variant in &union_def.variants {
        if let Some(wire) = &variant.wire_value
            && *wire != variant.variant_name
        {
            writeln!(out, "    #[serde(rename = \"{wire}\")]")?;
        }
        writeln!(out, "    {}({}),", variant.variant_name, variant.inner_type)?;
    }
    writeln!(out, "}}")
}

fn write_model_rs(
    entities: &[Entity],
    enum_name_map: &EnumNameMap,
    diagnostics: &mut Vec<Diagnostic>,
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
        writeln!(out, "pub struct {} {{", s.name)?;
        for field in &s.fields {
            // Resolve enum type name if this field uses a renamed enum
            let resolved_type = enum_name_map
                .get(&(s.name.clone(), field.rust_type.clone()))
                .cloned()
                .unwrap_or_else(|| field.rust_type.clone());
            let final_type = if field.is_optional {
                format!("Option<{resolved_type}>")
            } else {
                resolved_type
            };
            if let Some(default_val) = &field.default_value {
                // Only emit the serde attribute when `write_default_rs` will
                // actually emit the matching function; otherwise the attribute
                // would reference a nonexistent path and fail to compile.
                if format_default_literal(
                    default_val,
                    &field.rust_type,
                    field.is_optional,
                    enum_name_map,
                    &s.name,
                )
                .is_some()
                {
                    let struct_snake = to_snake_case(&s.name);
                    let fn_name = format!("{struct_snake}_{}", field.name);
                    writeln!(out, "    #[serde(default = \"crate::default::{fn_name}\")]")?;
                } else {
                    record(
                        diagnostics,
                        Severity::Degraded,
                        format!("{}.{}", s.name, field.name),
                        "default value",
                        "default value could not be rendered as a Rust literal; no `#[serde(default)]` was emitted",
                    );
                }
            }
            writeln!(
                out,
                "    pub {}: {final_type},",
                escape_keyword(&field.name)
            )?;
        }
        writeln!(out, "}}")?;
    }

    Ok(out)
}

/// Convert a JSON default value to a Rust literal string for the given type.
/// Returns `None` if the combination is unsupported.
fn format_default_literal(
    value: &serde_json::Value,
    rust_type: &str,
    is_optional: bool,
    enum_name_map: &EnumNameMap,
    struct_name: &str,
) -> Option<String> {
    let literal = match (value, rust_type) {
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
        (serde_json::Value::String(s), enum_type) => {
            // Inline enum: look up the resolved enum name and find the matching variant
            let fallback = enum_type.to_string();
            let resolved_enum = enum_name_map
                .get(&(struct_name.to_string(), enum_type.to_string()))
                .unwrap_or(&fallback);
            // Find the variant that matches this default value by checking all enums
            // The variant pair is (PascalCase, original_value)
            let variant = find_enum_variant_for_default(s, resolved_enum, enum_name_map)?;
            format!("{resolved_enum}::{variant}")
        }
        _ => return None,
    };
    if is_optional {
        Some(format!("Some({literal})"))
    } else {
        Some(literal)
    }
}

/// Find the PascalCase variant name for a default string value in an enum.
/// This is called from `format_default_literal` for inline enum fields.
fn find_enum_variant_for_default(
    default_str: &str,
    _resolved_enum_name: &str,
    _enum_name_map: &EnumNameMap,
) -> Option<String> {
    // Convert the default value to PascalCase to match the variant name.
    // This works because enum variants are generated as to_pascal_case(original_value)
    // and the original_value is what appears in the OpenAPI default.
    Some(crate::parse::to_pascal_case(default_str))
}

fn write_default_rs(
    entities: &[Entity],
    enum_name_map: &EnumNameMap,
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
    let needs_model_import = struct_fields_with_name.clone().any(|(name, fields)| {
        fields.iter().any(|f| {
            f.default_value.is_some()
                && enum_name_map.contains_key(&(name.clone(), f.rust_type.clone()))
        })
    });
    let model_import = if needs_model_import {
        "use crate::model::*;\n"
    } else {
        ""
    };

    let header = header_comment();
    let mut out = format!(
        "\
// {header}

{chrono_import}{uuid_import}{model_import}"
    );

    for entity in entities {
        let Entity::Struct(s) = entity else {
            continue;
        };
        let struct_snake = to_snake_case(&s.name);
        for field in &s.fields {
            let Some(default_val) = &field.default_value else {
                continue;
            };
            // Resolve the enum type name if applicable
            let resolved_type = enum_name_map
                .get(&(s.name.clone(), field.rust_type.clone()))
                .cloned()
                .unwrap_or_else(|| field.rust_type.clone());
            let return_type = if field.is_optional {
                format!("Option<{resolved_type}>")
            } else {
                resolved_type.clone()
            };
            let Some(literal) = format_default_literal(
                default_val,
                &field.rust_type,
                field.is_optional,
                enum_name_map,
                &s.name,
            ) else {
                continue;
            };
            let fn_name = format!("{struct_snake}_{}", field.name);
            writeln!(out)?;
            writeln!(out, "pub(crate) fn {fn_name}() -> {return_type} {{")?;
            writeln!(out, "    {literal}")?;
            writeln!(out, "}}")?;
        }
    }

    Ok(out)
}

fn write_validation_rs(entities: &[Entity], needs_regex: bool) -> Result<String, std::fmt::Error> {
    let regex_import = if needs_regex {
        "use regex::Regex;\n"
    } else {
        ""
    };
    let header = header_comment();
    let mut out = format!(
        "\
// {header}

{regex_import}use crate::model::*;

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
            Entity::Struct(s) => write_validation_impl(&mut out, &s.name, &s.fields)?,
            Entity::Enum(e) => {
                writeln!(out)?;
                writeln!(out, "impl Validation for {} {{}}", e.name)?;
            }
            Entity::Union(u) => write_union_validation_impl(&mut out, u)?,
        }
    }

    for (enum_name, _) in &inline_enums {
        writeln!(out)?;
        writeln!(out, "impl Validation for {enum_name} {{}}")?;
    }

    Ok(out)
}

/// Write the `impl Validation` block for a struct entity.
fn write_validation_impl(out: &mut String, name: &str, fields: &[Field]) -> std::fmt::Result {
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
        write_field_checks(out, field)?;
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
        writeln!(out, "impl Validation for {} {{}}", union_def.name)?;
        return Ok(());
    }
    writeln!(out, "impl Validation for {} {{", union_def.name)?;
    writeln!(
        out,
        "    fn validate(&self) -> Result<(), ValidationError> {{"
    )?;
    writeln!(out, "        match self {{")?;
    for variant in &union_def.variants {
        writeln!(
            out,
            "            Self::{}(inner) => inner.validate(),",
            variant.variant_name
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
    // VecNested emits a `for` loop, Array unique_items emits a block — neither is collapsible.
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
fn write_field_checks(out: &mut String, field: &Field) -> std::fmt::Result {
    let name = &field.name;
    let field_ident = escape_keyword(name);

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
    let default_mod = if needs_defaults { "mod default;\n" } else { "" };
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
fn escape_keyword(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

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
        assert!(v.contains("use regex::Regex;"), "missing regex import: {v}");
        let cargo = find_file(&krate, "Cargo.toml");
        assert!(
            cargo.contains("regex.workspace = true"),
            "missing regex dep: {cargo}"
        );

        Ok(())
    }

    /// A `pattern` containing brace quantifiers (e.g. `{1,14}`) must have its
    /// braces doubled in the `format!` error-message literal — but NOT in the
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
