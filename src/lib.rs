#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

mod error;
mod parse;
mod write;

pub use parse::parse;
pub use write::write;

use openapiv3::OpenAPI;

use error::Result;

pub struct Config {
    pub crate_name: String,
    /// If true, use `foo.workspace = true` for dependencies.
    /// If false, use fixed version numbers.
    pub use_workspace: bool,
}

pub struct GeneratedFile {
    /// Path relative to the generated crate root (e.g. `src/lib.rs`).
    pub path: &'static str,
    pub content: String,
}

pub struct GeneratedCrate {
    pub files: Vec<GeneratedFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// From `components.schemas` — derives Serialize + Deserialize.
    Schema,
    /// From operation query parameters — derives only Deserialize.
    Query,
}

#[derive(Debug, PartialEq)]
pub struct Field {
    /// Field name as it appears in the struct (e.g. `name`, `limit`).
    pub name: String,
    /// Base Rust type before Option wrapping (e.g. `String`, `i64`, `Vec<Foo>`).
    pub rust_type: String,
    /// Whether the field is wrapped in `Option<T>` (not required or nullable).
    pub is_optional: bool,
    /// Validation constraints extracted from the OpenAPI schema.
    pub constraints: Constraints,
}

#[derive(Debug, PartialEq)]
pub struct Entity {
    /// Struct name (e.g. `Foo`, `GetThingsQuery`).
    pub name: String,
    /// Whether this is a schema or query entity (affects derive macros).
    pub kind: EntityKind,
    /// Ordered list of fields.
    pub fields: Vec<Field>,
    /// String enums generated from inline enum fields.
    pub enums: Vec<EnumDef>,
}

#[derive(Debug, PartialEq)]
pub struct EnumDef {
    /// Enum name in PascalCase (e.g. `ChargingFrequency`).
    pub name: String,
    /// Variants in PascalCase with their original snake_case values.
    pub variants: Vec<(String, String)>,
}

/// Constraints extracted from a single field's OpenAPI schema.
#[derive(Debug, PartialEq)]
pub enum Constraints {
    /// No constraints — field needs no validation checks.
    None,
    String {
        min_length: Option<usize>,
        max_length: Option<usize>,
        pattern: Option<String>,
        enumeration: Vec<String>,
    },
    Integer {
        minimum: Option<i64>,
        maximum: Option<i64>,
        exclusive_minimum: bool,
        exclusive_maximum: bool,
        multiple_of: Option<i64>,
        enumeration: Vec<i64>,
    },
    Number {
        minimum: Option<f64>,
        maximum: Option<f64>,
        exclusive_minimum: bool,
        exclusive_maximum: bool,
        multiple_of: Option<f64>,
    },
    Array {
        min_items: Option<usize>,
        max_items: Option<usize>,
        unique_items: bool,
    },
    /// Field is a `$ref` to another generated struct — call `.validate()`.
    Nested,
    /// Field is `Vec<$ref>` — iterate and call `.validate()` on each element.
    VecNested,
}

impl Constraints {
    pub(crate) fn has_checks(&self) -> bool {
        match self {
            Constraints::None => false,
            Constraints::String {
                min_length,
                max_length,
                pattern,
                enumeration,
            } => {
                min_length.is_some()
                    || max_length.is_some()
                    || pattern.is_some()
                    || !enumeration.is_empty()
            }
            Constraints::Integer {
                minimum,
                maximum,
                exclusive_minimum,
                exclusive_maximum,
                multiple_of,
                enumeration,
            } => {
                minimum.is_some()
                    || maximum.is_some()
                    || *exclusive_minimum
                    || *exclusive_maximum
                    || multiple_of.is_some()
                    || !enumeration.is_empty()
            }
            Constraints::Number {
                minimum,
                maximum,
                exclusive_minimum,
                exclusive_maximum,
                multiple_of,
            } => {
                minimum.is_some()
                    || maximum.is_some()
                    || *exclusive_minimum
                    || *exclusive_maximum
                    || multiple_of.is_some()
            }
            Constraints::Array {
                min_items,
                max_items,
                unique_items,
            } => min_items.is_some() || max_items.is_some() || *unique_items,
            Constraints::Nested | Constraints::VecNested => true,
        }
    }

    /// Number of independent validation checks this constraint will emit.
    pub(crate) fn n_checks(&self) -> usize {
        match self {
            Constraints::None => 0,
            Constraints::String {
                min_length,
                max_length,
                pattern,
                enumeration,
            } => [
                min_length.is_some(),
                max_length.is_some(),
                pattern.is_some(),
                !enumeration.is_empty(),
            ]
            .iter()
            .filter(|&&x| x)
            .count(),
            Constraints::Integer {
                minimum,
                maximum,
                multiple_of,
                enumeration,
                ..
            } => [
                minimum.is_some(),
                maximum.is_some(),
                multiple_of.is_some(),
                !enumeration.is_empty(),
            ]
            .iter()
            .filter(|&&x| x)
            .count(),
            Constraints::Number {
                minimum,
                maximum,
                multiple_of,
                ..
            } => [minimum.is_some(), maximum.is_some(), multiple_of.is_some()]
                .iter()
                .filter(|&&x| x)
                .count(),
            Constraints::Array {
                min_items,
                max_items,
                unique_items,
            } => [min_items.is_some(), max_items.is_some(), *unique_items]
                .iter()
                .filter(|&&x| x)
                .count(),
            Constraints::Nested | Constraints::VecNested => 1,
        }
    }
}

/// Load an OpenAPI spec from a YAML string.
pub fn load_spec(yaml: &str) -> Result<OpenAPI> {
    Ok(serde_yaml::from_str(yaml)?)
}

/// Generate a complete crate from an OpenAPI spec and configuration.
pub fn generate(spec: &OpenAPI, config: &Config) -> Result<GeneratedCrate> {
    let entities = parse(spec);
    Ok(write(&entities, config)?)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn test_config() -> Config {
        Config {
            crate_name: "test_api".to_string(),
            use_workspace: true,
        }
    }

    fn file_content<'a>(crate_: &'a GeneratedCrate, path: &str) -> &'a str {
        &crate_
            .files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("missing file: {path}"))
            .content
    }

    /// Test that nested schemas are generated correctly and that validation errors are properly
    /// propagated with field paths (e.g. `child.name: length 0 is less than minimum 1`).
    #[test]
    fn nested_schema() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Parent:
      type: object
      required: [child]
      properties:
        child:
          $ref: "#/components/schemas/Child"
    Child:
      type: object
      required: [name]
      properties:
        name:
          type: string
          minLength: 1
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;

        assert_eq!(
            file_content(&crate_, "src/model.rs"),
            "\
// This file is @generated — do not edit manually.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parent {
    pub child: Child,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Child {
    pub name: String,
}
"
        );

        let validation = file_content(&crate_, "src/validation.rs");
        assert!(validation.starts_with("// This file is @generated"));
        assert!(validation.contains("pub struct ValidationError"));
        assert!(validation.contains("pub trait Validation"));
        assert!(validation.contains("impl Validation for Parent"));
        assert!(validation.contains("impl Validation for Child"));
        assert!(validation.contains("self.child.validate()"));
        assert!(validation.contains("self.name.chars().count() < 1"));

        Ok(())
    }

    /// Test that query parameters are generated correctly and that path parameters are
    /// not included in the query struct.
    #[test]
    fn path_and_query_parameters() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
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
  schemas: {}
"#;
        let spec = load_spec(yaml)?;
        let crate_ = generate(&spec, &test_config())?;

        assert_eq!(
            file_content(&crate_, "src/model.rs"),
            "\
// This file is @generated — do not edit manually.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct GetThingsQuery {
    pub limit: Option<i32>,
}
"
        );

        assert_eq!(
            file_content(&crate_, "src/validation.rs"),
            "\
// This file is @generated — do not edit manually.

use crate::model::*;

#[derive(Debug)]
pub struct ValidationError {
    pub details: Vec<String>,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, \"validation failed: {}\", self.details.join(\"; \"))
    }
}

impl std::error::Error for ValidationError {}

pub trait Validation {
    fn validate(&self) -> Result<(), ValidationError> {
        Ok(())
    }
}

impl<T: Validation> Validation for Vec<T> {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        for (i, item) in self.iter().enumerate() {
            if let Err(e) = item.validate() {
                for detail in e.details {
                    errors.push(format!(\"[{i}]: {detail}\"));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}

impl Validation for GetThingsQuery {}
"
        );

        Ok(())
    }

    /// Crate structure: all expected files, headers, Cargo.toml name, lib.rs re-exports.
    #[test]
    fn crate_structure() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
"#;
        let spec = load_spec(yaml)?;
        let crate_ = generate(&spec, &test_config())?;

        let paths: Vec<&str> = crate_.files.iter().map(|f| f.path).collect();
        assert_eq!(
            paths,
            vec![
                "Cargo.toml",
                "src/lib.rs",
                "src/validation.rs",
                "src/model.rs",
            ]
        );

        assert_eq!(
            file_content(&crate_, "Cargo.toml"),
            "\
# This file is @generated — do not edit manually.

[package]
name = \"test_api\"
version = \"0.1.0\"
edition = \"2024\"

[dependencies]
chrono = { workspace = true, features = [\"serde\"] }
serde = { workspace = true, features = [\"derive\"] }
serde_json.workspace = true

[dev-dependencies]
pretty_assertions.workspace = true
"
        );

        assert_eq!(
            file_content(&crate_, "src/lib.rs"),
            "\
// This file is @generated — do not edit manually.

mod model;
mod validation;

pub use model::*;
pub use validation::{Validation, ValidationError};
"
        );

        Ok(())
    }
}
