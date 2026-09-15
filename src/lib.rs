#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

mod diagnostic;
mod error;
mod ident;
mod parse;
mod write;

pub use diagnostic::{Diagnostic, Severity};
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
    /// Spec constructs that could not be fully generated. Empty when the whole
    /// spec was representable. See [`Diagnostic`].
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    /// From `components.schemas`, derives Serialize + Deserialize.
    Schema,
    /// From operation query parameters, derives only Deserialize.
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
    /// Default value from the OpenAPI schema, if present and supported.
    pub default_value: Option<serde_json::Value>,
    /// The `$ref` this field's type came from, when it came from one directly.
    /// Kept so a reference that resolves to nothing can be named in full.
    pub ref_target: Option<String>,
    /// Whether `rust_type` names an enum declared inline on this field, which
    /// the writer renames to keep it unique (`Language` becomes
    /// `GreetingLanguage`). A `$ref` field can carry that same name, so the
    /// name alone does not say which fields to rename.
    pub is_inline_enum: bool,
}

#[derive(Debug, PartialEq)]
pub enum Entity {
    /// A struct generated from an object schema or query parameters.
    Struct(StructDef),
    /// A standalone enum generated from a top-level string enum schema.
    Enum(EnumDef),
    /// A tagged/untagged union generated from a top-level `oneOf` schema.
    Union(UnionDef),
}

#[derive(Debug, PartialEq)]
pub struct StructDef {
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

/// A union type generated from a top-level `oneOf` schema.
///
/// When `tag` is `Some(prop)` (a `discriminator` was present), it maps to a
/// serde internally-tagged enum (`#[serde(tag = "prop")]`); otherwise it maps
/// to an untagged enum (`#[serde(untagged)]`).
///
/// Every variant of a tagged union is guaranteed to wrap a generated *struct*,
/// and that struct is guaranteed not to declare `prop` as a field, since serde emits
/// the tag key itself. Both invariants are established by the `resolve_unions`
/// post-pass in [`parse`], not by the per-schema parse.
#[derive(Debug, PartialEq)]
pub struct UnionDef {
    /// Enum name in PascalCase (e.g. `Pet`).
    pub name: String,
    /// One variant per `oneOf` member.
    pub variants: Vec<UnionVariant>,
    /// `Some(property_name)` for a discriminated (tagged) union, `None` for untagged.
    pub tag: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct UnionVariant {
    /// Variant name in PascalCase (e.g. `Dog`).
    pub variant_name: String,
    /// The referenced Rust type wrapped by this variant (e.g. `Dog`).
    pub inner_type: String,
    /// Tag value this variant carries on the wire, for a discriminated union.
    /// Taken from a `discriminator.mapping` entry when one points at the
    /// member, otherwise the member's schema name (what OpenAPI implies).
    /// `None` for an untagged union.
    pub wire_value: Option<String>,
}

/// Constraints extracted from a single field's OpenAPI schema.
#[derive(Debug, PartialEq)]
pub enum Constraints {
    /// No constraints: the field needs no validation checks.
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
    /// Field is a `$ref` to another generated struct: call `.validate()`.
    Nested,
    /// Field is `Vec<$ref>`: iterate and call `.validate()` on each element.
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
///
/// The returned [`GeneratedCrate::diagnostics`] lists every spec construct that
/// could not be fully generated (dropped or degraded). It is empty when the
/// whole spec was representable.
pub fn generate(spec: &OpenAPI, config: &Config) -> Result<GeneratedCrate> {
    let (entities, mut diagnostics) = parse(spec);
    let mut generated = write(&entities, config)?;
    // Parse-time diagnostics come first (spec order), then write-time ones.
    diagnostics.extend(std::mem::take(&mut generated.diagnostics));
    generated.diagnostics = diagnostics;

    // A fatal diagnostic means two spec names want one Rust name. Every other
    // diagnostic describes output the caller can still use; this one does not,
    // and no choice here would be better than the spec being fixed.
    let problems: Vec<Diagnostic> = generated
        .diagnostics
        .iter()
        .filter(|d| d.severity.is_fatal())
        .cloned()
        .collect();
    if !problems.is_empty() {
        return Err(error::Error::Unrepresentable(problems));
    }

    Ok(generated)
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

    /// Generate, expecting a fatal problem, and return the rendered error.
    fn spec_error(yaml: &str) -> String {
        let spec = load_spec(yaml).expect("spec should parse");
        let Err(err) = generate(&spec, &test_config()) else {
            panic!("generation should have been aborted");
        };
        assert!(
            matches!(err, error::Error::Unrepresentable(_)),
            "expected a name collision, got {err}"
        );
        err.to_string()
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
// This file is @generated. Do not edit manually.

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
// This file is @generated. Do not edit manually.

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
// This file is @generated. Do not edit manually.

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
# This file is @generated. Do not edit manually.

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
// This file is @generated. Do not edit manually.

mod model;
mod validation;

pub use model::*;
pub use validation::{Validation, ValidationError};
"
        );

        Ok(())
    }

    #[test]
    fn standalone_enum_schema() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Status:
      type: string
      enum: [ACTIVE, INACTIVE]
    Foo:
      type: object
      required: [status]
      properties:
        status:
          $ref: "#/components/schemas/Status"
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;

        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub enum Status {"),
            "standalone enum should be generated: {model}"
        );
        assert!(
            model.contains("pub status: Status,"),
            "field should reference standalone enum: {model}"
        );

        let validation = file_content(&crate_, "src/validation.rs");
        assert!(
            validation.contains("impl Validation for Status {}"),
            "standalone enum should get Validation impl: {validation}"
        );

        Ok(())
    }

    /// Scalar defaults (String, i32, f64, bool): non-nullable, non-required fields
    /// with defaults should be promoted from Option<T> to T. A default.rs file
    /// should be generated with the default-value functions.
    #[test]
    fn scalar_defaults() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Config:
      type: object
      required: [name]
      properties:
        name:
          type: string
          default: "default_name"
        count:
          type: integer
          format: int32
          default: 42
        rate:
          type: number
          default: 1.5
        enabled:
          type: boolean
          default: true
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // Non-required + non-nullable + has default → promoted to bare types
        assert!(
            model.contains("pub name: String,"),
            "required string with default should stay bare: {model}"
        );
        assert!(
            model.contains("pub count: i32,"),
            "non-required with default should be promoted: {model}"
        );
        assert!(
            model.contains("pub rate: f64,"),
            "non-required with default should be promoted: {model}"
        );
        assert!(
            model.contains("pub enabled: bool,"),
            "non-required with default should be promoted: {model}"
        );

        // serde attributes should reference crate::default::
        assert!(
            model.contains(r#"#[serde(default = "crate::default::config_name")]"#),
            "missing serde default attr for name: {model}"
        );
        assert!(
            model.contains(r#"#[serde(default = "crate::default::config_count")]"#),
            "missing serde default attr for count: {model}"
        );

        // default.rs should exist with functions
        let defaults = file_content(&crate_, "src/default.rs");
        assert!(
            defaults.contains("pub fn config_name() -> String"),
            "missing default fn for name: {defaults}"
        );
        assert!(
            defaults.contains(r#"String::from("default_name")"#),
            "missing default literal for name: {defaults}"
        );
        assert!(
            defaults.contains("pub fn config_count() -> i32"),
            "missing default fn for count: {defaults}"
        );
        assert!(
            defaults.contains("42"),
            "missing default literal for count: {defaults}"
        );
        assert!(
            defaults.contains("pub fn config_rate() -> f64"),
            "missing default fn for rate: {defaults}"
        );
        assert!(
            defaults.contains("1.5_f64"),
            "missing default literal for rate: {defaults}"
        );
        assert!(
            defaults.contains("pub fn config_enabled() -> bool"),
            "missing default fn for enabled: {defaults}"
        );

        // lib.rs should include mod default
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(
            lib.contains("pub mod default;"),
            "lib.rs should include default module: {lib}"
        );

        Ok(())
    }

    /// Nullable + default: field stays Option<T>, default fn returns Some(value).
    #[test]
    fn nullable_with_default() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Foo:
      type: object
      properties:
        label:
          type: string
          nullable: true
          default: "unknown"
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub label: Option<String>,"),
            "nullable field should stay Option: {model}"
        );

        let defaults = file_content(&crate_, "src/default.rs");
        assert!(
            defaults.contains("pub fn foo_label() -> Option<String>"),
            "return type should be Option: {defaults}"
        );
        assert!(
            defaults.contains(r#"Some(String::from("unknown"))"#),
            "should wrap in Some: {defaults}"
        );

        Ok(())
    }

    /// DateTime, NaiveDate, Uuid defaults use .parse()/.parse_str() with .expect().
    #[test]
    fn parsed_type_defaults() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Event:
      type: object
      required: [name]
      properties:
        name:
          type: string
        created_at:
          type: string
          format: date-time
          default: "2024-01-01T00:00:00Z"
        event_date:
          type: string
          format: date
          default: "2024-06-15"
        event_id:
          type: string
          format: uuid
          default: "550e8400-e29b-41d4-a716-446655440000"
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let defaults = file_content(&crate_, "src/default.rs");

        assert!(
            defaults.contains("pub fn event_created_at() -> DateTime<Utc>"),
            "missing DateTime default fn: {defaults}"
        );
        assert!(
            defaults.contains(r#""2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().expect("hardcoded default from OpenAPI spec")"#),
            "missing DateTime parse: {defaults}"
        );
        assert!(
            defaults.contains("pub fn event_event_date() -> NaiveDate"),
            "missing NaiveDate default fn: {defaults}"
        );
        assert!(
            defaults.contains(
                r#""2024-06-15".parse::<NaiveDate>().expect("hardcoded default from OpenAPI spec")"#
            ),
            "missing NaiveDate parse: {defaults}"
        );
        assert!(
            defaults.contains("pub fn event_event_id() -> Uuid"),
            "missing Uuid default fn: {defaults}"
        );
        assert!(
            defaults.contains(r#"Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").expect("hardcoded default from OpenAPI spec")"#),
            "missing Uuid parse_str: {defaults}"
        );

        // Imports in default.rs
        assert!(
            defaults.contains("use chrono::{DateTime,"),
            "missing chrono import in default.rs: {defaults}"
        );
        assert!(
            defaults.contains("use uuid::Uuid;"),
            "missing uuid import in default.rs: {defaults}"
        );

        Ok(())
    }

    /// Inline enum defaults: the default string is mapped to the variant name.
    #[test]
    fn inline_enum_default() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Greeting:
      type: object
      required: [message]
      properties:
        message:
          type: string
        language:
          type: string
          enum: [en, de, fr]
          default: en
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let defaults = file_content(&crate_, "src/default.rs");

        assert!(
            defaults.contains("pub fn greeting_language() -> GreetingLanguage"),
            "missing enum default fn: {defaults}"
        );
        assert!(
            defaults.contains("GreetingLanguage::En"),
            "missing enum variant in default: {defaults}"
        );

        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub language: GreetingLanguage,"),
            "enum field with default should be promoted to bare type: {model}"
        );

        Ok(())
    }

    /// Unsupported default types (Vec) should be silently ignored.
    #[test]
    fn unsupported_default_ignored() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Foo:
      type: object
      properties:
        tags:
          type: array
          items:
            type: string
          default: ["a", "b"]
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // Vec default is unsupported → field stays Option<Vec<String>>
        assert!(
            model.contains("pub tags: Option<Vec<String>>,"),
            "unsupported default should leave field as Option: {model}"
        );

        // No default.rs should be generated (no supported defaults)
        assert!(
            crate_.files.iter().all(|f| f.path != "src/default.rs"),
            "default.rs should not be generated when no supported defaults exist"
        );

        // lib.rs should NOT include mod default
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(
            !lib.contains("mod default;"),
            "lib.rs should not include default module: {lib}"
        );

        Ok(())
    }

    /// Query parameters with defaults should also be promoted.
    #[test]
    fn query_param_with_default() -> Result<()> {
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
            default: 20
      responses:
        "200":
          description: OK
components:
  schemas: {}
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // limit has default → promoted from Option<i32> to i32
        assert!(
            model.contains("pub limit: i32,"),
            "query param with default should be promoted: {model}"
        );
        assert!(
            model.contains(r#"#[serde(default = "crate::default::get_things_query_limit")]"#),
            "missing serde default attr: {model}"
        );

        let defaults = file_content(&crate_, "src/default.rs");
        assert!(
            defaults.contains("pub fn get_things_query_limit() -> i32"),
            "missing default fn for query param: {defaults}"
        );

        Ok(())
    }

    /// No defaults in spec → no default.rs generated, lib.rs unchanged.
    #[test]
    fn no_defaults_no_default_file() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Simple:
      type: object
      required: [name]
      properties:
        name:
          type: string
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        assert!(
            crate_.files.iter().all(|f| f.path != "src/default.rs"),
            "default.rs should not exist without defaults"
        );
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(
            !lib.contains("mod default;"),
            "no default module when no defaults: {lib}"
        );

        Ok(())
    }

    /// Both members declare the discriminator property `petType`, the way real
    /// specs do, `Pet` must absorb it into the serde tag rather than leaving it
    /// on the structs.
    const ONE_OF_SPEC: &str = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
          minLength: 1
    Dog:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
    Pet:
      oneOf:
        - $ref: '#/components/schemas/Cat'
        - $ref: '#/components/schemas/Dog'
      discriminator:
        propertyName: petType
        mapping:
          cat: '#/components/schemas/Cat'
          dog: '#/components/schemas/Dog'
    Shape:
      oneOf:
        - $ref: '#/components/schemas/Cat'
        - $ref: '#/components/schemas/Dog'
"##;

    /// A top-level `oneOf` with a discriminator + mapping becomes an internally
    /// tagged enum; without a discriminator it becomes an untagged enum. Both
    /// get a `Validation` impl that delegates to the active variant.
    #[test]
    fn one_of_union_schema() -> Result<()> {
        let crate_ = generate(&load_spec(ONE_OF_SPEC)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // Discriminated union → internally tagged, with renamed variants.
        assert!(
            model.contains("#[serde(tag = \"petType\")]\npub enum Pet {"),
            "discriminated union should be internally tagged: {model}"
        );
        assert!(
            model.contains("#[serde(rename = \"cat\")]\n    Cat(Cat),"),
            "mapping should rename the Cat variant: {model}"
        );
        assert!(
            model.contains("#[serde(rename = \"dog\")]\n    Dog(Dog),"),
            "mapping should rename the Dog variant: {model}"
        );

        // Undiscriminated union → untagged.
        assert!(
            model.contains("#[serde(untagged)]\npub enum Shape {"),
            "union without discriminator should be untagged: {model}"
        );

        // serde writes the `petType` key itself for the tagged union, so the
        // member structs must not also declare it, or serializing emits
        // a duplicate key and deserializing fails with `missing field petType`.
        assert!(
            !model.contains("pub petType"),
            "discriminator property should be absorbed into the tag: {model}"
        );
        assert!(
            model.contains("pub struct Cat {\n    pub name: String,\n}"),
            "Cat should keep its remaining properties: {model}"
        );

        let validation = file_content(&crate_, "src/validation.rs");
        assert!(
            validation.contains("impl Validation for Pet {"),
            "union should get a Validation impl: {validation}"
        );
        assert!(
            validation.contains("Self::Cat(inner) => inner.validate(),")
                && validation.contains("Self::Dog(inner) => inner.validate(),"),
            "union validation should delegate to each variant: {validation}"
        );

        Ok(())
    }

    /// `generate()` surfaces a diagnostic for every construct it cannot fully
    /// represent, so library callers (not just the CLI) can see what was lost.
    #[test]
    fn generate_reports_diagnostics() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths:
  /things:
    post:
      operationId: createThing
      requestBody:
        content:
          application/json:
            schema:
              type: object
              properties:
                note:
                  type: string
      parameters:
        - name: X-Trace
          in: header
          required: false
          schema:
            type: string
      responses:
        "200":
          description: OK
components:
  schemas:
    Base:
      type: object
      properties:
        id:
          type: string
    Derived:
      allOf:
        - $ref: "#/components/schemas/Base"
    Pet:
      type: object
      properties:
        metadata:
          type: object
          properties:
            key:
              type: string
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;

        let has = |construct: &str, severity: Severity| {
            crate_
                .diagnostics
                .iter()
                .any(|d| d.construct == construct && d.severity == severity)
        };

        assert!(has("allOf", Severity::Dropped), "{:?}", crate_.diagnostics);
        assert!(
            has("inline object", Severity::Degraded),
            "{:?}",
            crate_.diagnostics
        );
        assert!(
            has("header parameter", Severity::Dropped),
            "{:?}",
            crate_.diagnostics
        );
        assert!(
            has("request body", Severity::Dropped),
            "{:?}",
            crate_.diagnostics
        );

        // The `Base` and `Pet` object schemas ARE generated despite the losses.
        let model = file_content(&crate_, "src/model.rs");
        assert!(model.contains("pub struct Base"));
        assert!(model.contains("pub struct Pet"));

        Ok(())
    }

    /// A default the spec declares but the writer cannot render (here `1.5` for
    /// an `i32`) produces no default function, so no `default.rs` module should
    /// be emitted for it, or an empty module would be left behind.
    #[test]
    fn unrenderable_default_omits_default_module() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Foo:
      type: object
      properties:
        tags:
          type: array
          items:
            type: string
          default: ["a"]
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;

        assert!(
            crate_.files.iter().all(|f| f.path != "src/default.rs"),
            "no renderable default → no default.rs"
        );
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(!lib.contains("mod default;"), "no default module: {lib}");
        assert!(
            crate_
                .diagnostics
                .iter()
                .any(|d| d.construct == "default value"),
            "the dropped default should still be reported: {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// Absorbing a discriminator into the serde tag is a representation choice,
    /// not a loss, so it is never reported, including when the member is also
    /// reached directly, where the key is redundant with the field's static type.
    #[test]
    fn absorbing_discriminator_is_silent() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Cat:
      type: object
      required: [petType, name]
      properties:
        petType:
          type: string
        name:
          type: string
    Pet:
      oneOf:
        - $ref: '#/components/schemas/Cat'
      discriminator:
        propertyName: petType
    Household:
      type: object
      properties:
        resident:
          $ref: '#/components/schemas/Cat'
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        assert!(
            crate_.diagnostics.is_empty(),
            "expected no diagnostics, got {:?}",
            crate_.diagnostics
        );

        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub struct Cat {\n    pub name: String,\n}"),
            "petType belongs to the tag, not the struct: {model}"
        );

        Ok(())
    }

    /// A fully-supported spec yields an empty diagnostics list.
    #[test]
    fn generate_clean_spec_has_no_diagnostics() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Thing:
      type: object
      required: [name]
      properties:
        name:
          type: string
          minLength: 1
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        assert!(
            crate_.diagnostics.is_empty(),
            "expected no diagnostics, got {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// A `default` that contradicts its own type aborts the run, and the
    /// message names every offending property at once so one pass over the
    /// spec fixes them all.
    #[test]
    fn mismatched_defaults_abort_generation() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Reading:
      type: object
      properties:
        count:
          type: integer
          default: 1.5
        takenAt:
          type: string
          format: date-time
          default: "yesterday"
"#;
        assert_eq!(
            spec_error(yaml),
            r#"2 fatal problems in the spec

  Reading.count
    default value 1.5 is not valid for type `i64`: not an integer

  Reading.takenAt
    default value "yesterday" is not valid for type `DateTime<Utc>`: not an RFC 3339 date-time

Fix the spec, then re-run. No files were written."#
        );

        Ok(())
    }

    /// Enum values are arbitrary spec strings: they start with digits or carry
    /// punctuation. Each has to become a legal variant, with the value itself
    /// preserved by a `#[serde(rename)]`.
    #[test]
    fn enum_values_that_are_not_identifiers() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        resolution:
          type: string
          enum: ["10min", "1h", "P1D", "with space", "x/y", "self"]
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // `P1D` is already usable, and `self` cannot be a raw identifier.
        assert!(
            model.contains(
                r#"pub enum SeriesResolution {
    #[serde(rename = "10min")]
    Variant10min,
    #[serde(rename = "1h")]
    Variant1h,
    #[serde(rename = "P1D")]
    P1D,
    #[serde(rename = "with space")]
    WithSpace,
    #[serde(rename = "x/y")]
    XY,
    #[serde(rename = "self")]
    Self_,
}"#
            ),
            "generated model:\n{model}"
        );

        // Sanitizing loses nothing: the wire value survives in the rename.
        assert!(
            crate_.diagnostics.is_empty(),
            "expected no diagnostics, got {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// Two values that sanitize to the same variant have no honest resolution:
    /// dropping one makes it undeserializable, suffixing one puts a variant in
    /// the API that appears nowhere in the spec. Generation fails instead.
    #[test]
    fn colliding_enum_values_abort() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        kind:
          type: string
          enum: ["a.b", "a-b"]
"#;
        assert_eq!(
            spec_error(yaml),
            r#"1 fatal problem in the spec

  Series.kind
    enum values "a.b" and "a-b" would both become the Rust enum variant `AB`

Fix the spec, then re-run. No files were written."#
        );

        Ok(())
    }

    /// The `Variant` prefix is a plain string, so a value needing it can land on a
    /// value that spells it out. That is the same clash as `a.b` vs `a-b`, and
    /// it aborts the same way.
    #[test]
    fn a_prefixed_variant_colliding_with_a_spelled_out_value_aborts() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        resolution:
          type: string
          enum: ["10min", "Variant10min"]
"#;
        assert_eq!(
            spec_error(yaml),
            r#"1 fatal problem in the spec

  Series.resolution
    enum values "10min" and "Variant10min" would both become the Rust enum variant `Variant10min`

Fix the spec, then re-run. No files were written."#
        );

        Ok(())
    }

    /// A value listed twice would emit two variants renamed to the same string,
    /// which serde rejects at compile time.
    #[test]
    fn repeated_enum_value_is_dropped() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        kind:
          type: string
          enum: ["a", "a"]
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        assert_eq!(
            model.matches(r#"#[serde(rename = "a")]"#).count(),
            1,
            "the repeated value should be emitted once: {model}"
        );
        assert!(
            crate_
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Dropped && d.reason.contains("listed twice")),
            "the repeat should be reported: {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// The default is resolved through the enum's own variant list rather than
    /// re-derived from the string, so a sanitized value still resolves, and a
    /// change to variant naming cannot leave the default naming a variant that
    /// does not exist.
    #[test]
    fn inline_enum_default_uses_the_sanitized_variant() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        resolution:
          type: string
          enum: ["10min", "1h"]
          default: "1h"
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let defaults = file_content(&crate_, "src/default.rs");

        assert!(
            defaults.contains("SeriesResolution::Variant1h"),
            "default should name the sanitized variant: {defaults}"
        );

        Ok(())
    }

    /// Property names are spec strings too. An unusable one is sanitized into a
    /// field identifier and renamed back on the wire; everything derived from
    /// the field (default function, validation accessor) follows the
    /// identifier, while messages keep the spec's name.
    #[test]
    fn property_names_that_are_not_identifiers() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        "10min":
          type: string
          default: "x"
        "first-name":
          type: string
          maxLength: 5
        "self":
          type: string
        "type":
          type: string
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // `type` needs no rename: serde strips the `r#`.
        assert!(
            model.contains(
                r##"pub struct Series {
    #[serde(rename = "10min")]
    #[serde(default = "crate::default::series_10min")]
    pub _10min: String,
    #[serde(rename = "first-name")]
    pub first_name: Option<String>,
    #[serde(rename = "self")]
    pub self_: Option<String>,
    pub r#type: Option<String>,
}"##
            ),
            "generated model:\n{model}"
        );

        // The default function follows the field identifier, not the property.
        let defaults = file_content(&crate_, "src/default.rs");
        assert!(
            defaults.contains(
                r#"pub fn series_10min() -> String {
    String::from("x")
}"#
            ),
            "generated defaults:\n{defaults}"
        );

        // Checks reach the field by its identifier; messages keep the spec's name.
        let validation = file_content(&crate_, "src/validation.rs");
        assert!(
            validation.contains(
                r#"impl Validation for Series {
    fn validate(&self) -> Result<(), ValidationError> {
        let mut errors = Vec::new();
        if let Some(val) = &self.first_name
            && (*val).chars().count() > 5
        {
            errors.push(format!(
                "first-name: length {} exceeds maximum 5",
                (*val).chars().count()
            ));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError { details: errors })
        }
    }
}"#
            ),
            "generated validation:\n{validation}"
        );
        // Sanitizing a property name is not a loss: the rename carries it.
        assert!(
            crate_.diagnostics.is_empty(),
            "expected no diagnostics, got {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// A spec written in camelCase generates snake_case fields: rustc lints a
    /// field that is not snake_case, so passing the name through would make the
    /// *generated* crate warn. The wire name comes back via the rename.
    #[test]
    fn camel_case_properties_become_snake_case_fields() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths:
  /users:
    get:
      operationId: listUsers
      parameters:
        - name: pageSize
          in: query
          schema:
            type: integer
      responses:
        '200':
          description: ok
components:
  schemas:
    User:
      type: object
      properties:
        firstName:
          type: string
        userID:
          type: string
        HTTPProxyURL:
          type: string
        already_snake:
          type: string
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // An acronym is one word, not one word per letter, and a name that is
        // already snake_case needs no rename.
        assert!(
            model.contains(
                r#"pub struct User {
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "userID")]
    pub user_id: Option<String>,
    #[serde(rename = "HTTPProxyURL")]
    pub http_proxy_url: Option<String>,
    pub already_snake: Option<String>,
}"#
            ),
            "generated model:\n{model}"
        );
        // Query parameters are the same, and the rename is what keeps the URL
        // parameter readable.
        assert!(
            model.contains(
                r#"pub struct ListUsersQuery {
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
}"#
            ),
            "generated model:\n{model}"
        );
        // Casing is not a loss: the wire name survives in the rename.
        assert!(
            crate_.diagnostics.is_empty(),
            "expected no diagnostics, got {:?}",
            crate_.diagnostics
        );

        Ok(())
    }

    /// Two properties of one struct that sanitize to the same field identifier
    /// abort for the same reason as two enum values do.
    #[test]
    fn colliding_property_names_abort() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Series:
      type: object
      properties:
        "first-name":
          type: string
        "first.name":
          type: string
"#;
        let message = spec_error(yaml);
        assert!(
            message.contains(
                r#"properties "first-name" and "first.name" would both become the Rust struct field `first_name`"#
            ),
            "the error should name both properties: {message}"
        );

        Ok(())
    }

    /// Schema names become type names, at the definition and at every `$ref`
    /// that reaches them.
    #[test]
    fn schema_names_that_are_not_identifiers() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    3d-model:
      type: object
      properties:
        size:
          type: number
    Holder:
      type: object
      properties:
        model:
          $ref: '#/components/schemas/3d-model'
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // The `$ref` resolves to the same name the definition was given.
        assert!(
            model.contains(
                r#"pub struct Type3dModel {
    pub size: Option<f64>,
}"#
            ),
            "generated model:\n{model}"
        );
        assert!(
            model.contains(
                r#"pub struct Holder {
    pub model: Option<Type3dModel>,
}"#
            ),
            "generated model:\n{model}"
        );

        Ok(())
    }

    /// Two schema names that map to one type name are worse still: the `$ref`s
    /// to them are indistinguishable, so there is not even a wrong answer to
    /// pick.
    #[test]
    fn schemas_that_map_to_the_same_type_name_abort() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    foo-bar:
      type: object
      properties:
        a:
          type: string
    foo_bar:
      type: object
      properties:
        b:
          type: string
"#;
        let message = spec_error(yaml);
        assert!(
            message.contains(
                r#"schemas "foo-bar" and "foo_bar" would both become the Rust type `FooBar`"#
            ),
            "the error should name both schemas: {message}"
        );

        Ok(())
    }

    /// A pathological spec: every name is hostile. Nothing here is expected to
    /// be pretty: what matters is that generation completes (the writer
    /// asserts every identifier it emits) and that the wire strings put back by
    /// `#[serde(rename)]` are correctly escaped.
    #[test]
    fn hostile_names_still_generate() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Odd:
      type: object
      properties:
        "say \"hi\"":
          type: string
        "back\\slash":
          type: string
        "é":
          type: string
        kind:
          type: string
          enum: ['say "hi"', "0"]
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");

        // Quotes and backslashes survive into the rename, escaped; a non-ASCII
        // name is already a valid identifier.
        assert!(
            model.contains(
                r#"pub struct Odd {
    #[serde(rename = "say \"hi\"")]
    pub say_hi: Option<String>,
    #[serde(rename = "back\\slash")]
    pub back_slash: Option<String>,
    pub é: Option<String>,
    pub kind: Option<OddKind>,
}"#
            ),
            "generated model:\n{model}"
        );
        assert!(
            model.contains(
                r#"pub enum OddKind {
    #[serde(rename = "say \"hi\"")]
    SayHi,
    #[serde(rename = "0")]
    Variant0,
}"#
            ),
            "generated model:\n{model}"
        );

        Ok(())
    }

    /// A name with nothing to build an identifier from (`""`, `"!!!"`, `"_"`)
    /// leaves the generator no name to derive. `Empty` or `unnamed` would say
    /// nothing about what the item holds, so this aborts like a collision does.
    #[test]
    fn names_with_nothing_to_build_on_abort() -> Result<()> {
        let cases = [
            (
                "an enum value",
                r#"
        kind:
          type: string
          enum: ["ok", ""]
"#,
                r#"enum value "" has nothing a Rust name can be built from"#,
            ),
            (
                "a property name",
                r#"
        "!!!":
          type: string
"#,
                r#"property name "!!!" has nothing a Rust name can be built from"#,
            ),
            (
                "a property named `_`",
                r#"
        "_":
          type: string
"#,
                r#"property name "_" has nothing a Rust name can be built from"#,
            ),
        ];

        for (what, properties, expected) in cases {
            let yaml = format!(
                r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {{}}
components:
  schemas:
    Series:
      type: object
      properties:{properties}"#
            );
            let message = spec_error(&yaml);
            assert!(
                message.contains(expected),
                "{what} should be reported: {message}"
            );
        }

        Ok(())
    }

    /// A `$ref` to a schema that is not in the spec leaves the field naming a
    /// type nothing defines. The generator cannot invent it, so it aborts
    /// rather than emit a crate that will not compile.
    #[test]
    fn a_ref_to_a_missing_schema_aborts() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Holder:
      type: object
      properties:
        gone:
          $ref: '#/components/schemas/Nope'
"#;
        let message = spec_error(yaml);
        assert!(
            message.contains(
                r##"$ref "#/components/schemas/Nope" names a schema that does not exist"##
            ),
            "the reference should be named in full: {message}"
        );

        Ok(())
    }

    /// Same for a `$ref` to a schema that exists but generated nothing, whether
    /// the field holds it directly or in an array.
    #[test]
    fn a_ref_to_an_ungenerated_schema_aborts() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
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
    Holder:
      type: object
      properties:
        derived:
          $ref: '#/components/schemas/Derived'
        many:
          type: array
          items:
            $ref: '#/components/schemas/Derived'
"#;
        let message = spec_error(yaml);
        for field in ["Holder.derived", "Holder.many"] {
            assert!(
                message.contains(field),
                "{field} should be reported: {message}"
            );
        }
        assert_eq!(
            message
                .matches(
                    r##"$ref "#/components/schemas/Derived" names schema `Derived`, which produced no type"##
                )
                .count(),
            2,
            "an array element names its own $ref too: {message}"
        );

        Ok(())
    }

    /// A schema name is the same case, one level up.
    #[test]
    fn a_schema_name_with_nothing_to_build_on_aborts() -> Result<()> {
        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    "!!!":
      type: object
      properties:
        a:
          type: string
"#;
        let message = spec_error(yaml);
        assert!(
            message.contains(r#"schema name "!!!" has nothing a Rust name can be built from"#),
            "the schema should be reported: {message}"
        );

        Ok(())
    }
}
