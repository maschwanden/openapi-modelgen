use std::fmt::Write;

use crate::{
    Config, Constraints, Entity, EntityKind, EnumDef, Field, GeneratedCrate, GeneratedFile,
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
    let needs_regex = entities.iter().any(|e| {
        e.fields.iter().any(|f| {
            matches!(
                &f.constraints,
                Constraints::String {
                    pattern: Some(_),
                    ..
                }
            )
        })
    });

    let needs_uuid = entities
        .iter()
        .any(|e| e.fields.iter().any(|f| f.rust_type == "Uuid"));

    Ok(GeneratedCrate {
        files: vec![
            GeneratedFile {
                path: "Cargo.toml",
                content: generate_cargo_toml(config, needs_regex, needs_uuid),
            },
            GeneratedFile {
                path: "src/lib.rs",
                content: generate_lib_rs(),
            },
            GeneratedFile {
                path: "src/validation.rs",
                content: write_validation_rs(entities, needs_regex)?,
            },
            GeneratedFile {
                path: "src/model.rs",
                content: write_model_rs(entities)?,
            },
        ],
    })
}

fn header_comment() -> &'static str {
    "This file is @generated — do not edit manually."
}

fn write_model_rs(entities: &[Entity]) -> Result<String, std::fmt::Error> {
    let needs_uuid = entities
        .iter()
        .any(|e| e.fields.iter().any(|f| f.rust_type == "Uuid"));

    let uuid_import = if needs_uuid { "\nuse uuid::Uuid;" } else { "" };

    // -- Resolve enum names.
    // Always prefix inline enums with their parent entity name (e.g. `LoginSuccessResponseStatus`).
    // Deduplicate: if two prefixed enums have identical variants, reuse the first one.
    let mut enum_name_map: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();
    let mut final_enums: Vec<(String, &EnumDef)> = Vec::new();
    // variants → first emitted enum name (for deduplication across entities)
    let mut variants_to_name: std::collections::HashMap<Vec<String>, String> =
        std::collections::HashMap::new();

    for entity in entities {
        for enum_def in &entity.enums {
            let variants: Vec<String> = enum_def.variants.iter().map(|p| p.1.clone()).collect();

            if let Some(existing_name) = variants_to_name.get(&variants) {
                // Reuse existing enum with same variants
                enum_name_map.insert(
                    (entity.name.clone(), enum_def.name.clone()),
                    existing_name.clone(),
                );
            } else {
                let prefixed = format!("{}{}", entity.name, enum_def.name);
                enum_name_map.insert(
                    (entity.name.clone(), enum_def.name.clone()),
                    prefixed.clone(),
                );
                variants_to_name.insert(variants, prefixed.clone());
                final_enums.push((prefixed, enum_def));
            }
        }
    }

    let header = header_comment();
    let mut out = format!(
        "\
// {header}

use chrono::{{DateTime, NaiveDate, Utc}};
use serde::{{Deserialize, Serialize}};{uuid_import}
"
    );

    // Emit enums
    for (enum_name, enum_def) in &final_enums {
        writeln!(out)?;
        writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]")?;
        writeln!(out, "pub enum {enum_name} {{")?;
        for (variant, original) in &enum_def.variants {
            writeln!(out, "    #[serde(rename = \"{original}\")]")?;
            writeln!(out, "    {variant},")?;
        }
        writeln!(out, "}}")?;
    }

    for entity in entities {
        writeln!(out)?;
        match entity.kind {
            EntityKind::Schema => {
                writeln!(out, "#[derive(Debug, Clone, Serialize, Deserialize)]")?;
            }
            EntityKind::Query => {
                writeln!(out, "#[derive(Debug, Clone, Deserialize)]")?;
            }
        }
        writeln!(out, "pub struct {} {{", entity.name)?;
        for field in &entity.fields {
            // Resolve enum type name if this field uses a renamed enum
            let resolved_type = enum_name_map
                .get(&(entity.name.clone(), field.rust_type.clone()))
                .cloned()
                .unwrap_or_else(|| field.rust_type.clone());
            let final_type = if field.is_optional {
                format!("Option<{resolved_type}>")
            } else {
                resolved_type
            };
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

    for entity in entities {
        write_validation_impl(&mut out, entity)?;
    }

    Ok(out)
}

/// Write the `impl Validation` block for an entity.
fn write_validation_impl(out: &mut String, entity: &Entity) -> std::fmt::Result {
    let has_any_checks = entity.fields.iter().any(|f| f.constraints.has_checks());

    writeln!(out)?;

    if !has_any_checks {
        writeln!(out, "impl Validation for {} {{}}", entity.name)?;
        return Ok(());
    }

    writeln!(out, "impl Validation for {} {{", entity.name)?;

    writeln!(
        out,
        "    fn validate(&self) -> Result<(), ValidationError> {{"
    )?;
    writeln!(out, "        let mut errors = Vec::new();")?;

    for field in &entity.fields {
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
                write_error_push(
                    out,
                    indent,
                    &format!("\"{name}: value '{{}}' does not match pattern '{escaped}'\""),
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

fn generate_lib_rs() -> String {
    let header = header_comment();
    format!(
        "\
// {header}

mod model;
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
    use crate::{Config, Constraints, EntityKind, Field};

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
        Entity {
            name: name.into(),
            kind,
            fields: vec![field],
            enums: vec![],
        }
    }

    fn required_field(name: &str, rust_type: &str, constraints: Constraints) -> Field {
        Field {
            name: name.into(),
            rust_type: rust_type.into(),
            is_optional: false,
            constraints,
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
