#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]

mod common;
mod config;
mod diagnostics;
mod error;
mod manifest;
mod model;
mod server;

use std::collections::HashMap;

use openapiv3::OpenAPI;
use serde::Deserialize;

pub use config::{FileConfig, check_target_deps};
pub use diagnostics::{Diagnostic, Severity};
pub use error::Result;
pub use model::parse::{parse, parse_with_diagnostics};
pub use model::{
    AliasDef, Constraints, Entity, EntityKind, EnumDef, Field, ResponseEnumDef, StructDef,
    UnionDef, UnionVariant,
};
pub use server::Route;

/// How a server operation is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServerStyle {
    /// Emitted as a method on the generated `Api` trait (compile-enforced).
    #[default]
    Strict,
    /// Excluded from the generated trait — the user wires it by hand.
    Manual,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub crate_name: String,
    /// If true, use `foo.workspace = true` for dependencies.
    /// If false, use fixed version numbers.
    pub use_workspace: bool,
    /// Generate `model.rs` / `validation.rs` (+ `default.rs`).
    pub emit_model: bool,
    /// Generate `src/server.rs` with the framework-agnostic `Api` trait.
    pub emit_server: bool,
    /// Generate the axum adapter module inside `server.rs`.
    pub emit_axum: bool,
    /// Project-wide default server style.
    pub server_style: ServerStyle,
    /// Per-operation style overrides, keyed by `operationId`.
    pub operation_styles: HashMap<String, ServerStyle>,
}

impl Config {
    /// A model-only config (server generation off) with default styles.
    pub fn new(crate_name: String, use_workspace: bool) -> Self {
        Self {
            crate_name,
            use_workspace,
            emit_model: true,
            emit_server: false,
            emit_axum: false,
            server_style: ServerStyle::Strict,
            operation_styles: HashMap::new(),
        }
    }
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
    let mut diagnostics = Vec::new();
    let entities = if config.emit_model {
        parse_with_diagnostics(spec, &mut diagnostics)
    } else {
        Vec::new()
    };
    let routes = if config.emit_server {
        server::parse::parse_routes(spec, config, &mut diagnostics)
    } else {
        Vec::new()
    };
    let mut generated = assemble(&entities, &routes, config)?;
    // Parse-time diagnostics come first (spec order), then write-time ones.
    diagnostics.append(&mut generated.diagnostics);
    generated.diagnostics = diagnostics;
    Ok(generated)
}

/// Assemble the generated crate from already-parsed entities and routes.
///
/// Split out from [`generate`] so tests can drive the writers directly with
/// hand-built entities/routes.
pub(crate) fn assemble(
    entities: &[Entity],
    routes: &[Route],
    config: &Config,
) -> std::result::Result<GeneratedCrate, std::fmt::Error> {
    let struct_fields = entities.iter().filter_map(|e| match e {
        Entity::Struct(s) => Some(s.fields.as_slice()),
        Entity::Enum(_) | Entity::Union(_) | Entity::Alias(_) | Entity::ResponseEnum(_) => None,
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

    let (enum_name_map, _) = model::write::resolve_inline_enums(entities);

    // The axum adapter needs `axum-extra` only when it emits a query extractor.
    let needs_axum_query = config.emit_axum && routes.iter().any(|r| r.query_type.is_some());

    let mut diagnostics = Vec::new();
    // Render every field default once; both model.rs and default.rs consume the
    // result, and unrenderable defaults are reported here (a single site).
    let default_literals =
        model::write::compute_default_literals(entities, &enum_name_map, &mut diagnostics);

    let mut files = vec![
        GeneratedFile {
            path: "Cargo.toml",
            content: manifest::generate_cargo_toml(
                config,
                needs_regex,
                needs_uuid,
                needs_axum_query,
            ),
        },
        GeneratedFile {
            path: "src/lib.rs",
            content: manifest::generate_lib_rs(config, needs_defaults),
        },
    ];

    if config.emit_model {
        files.push(GeneratedFile {
            path: "src/validation.rs",
            content: model::write::validation_rs(entities, needs_regex)?,
        });
        files.push(GeneratedFile {
            path: "src/model.rs",
            content: model::write::model_rs(entities, &enum_name_map, &default_literals)?,
        });
        if needs_defaults {
            files.push(GeneratedFile {
                path: "src/default.rs",
                content: model::write::default_rs(entities, &enum_name_map, &default_literals)?,
            });
        }
    }

    if config.emit_server {
        files.push(GeneratedFile {
            path: "src/server.rs",
            content: server::server_rs(routes, config.emit_axum)?,
        });
    }

    Ok(GeneratedCrate { files, diagnostics })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn test_config() -> Config {
        Config::new("test_api".to_string(), true)
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

    /// A top-level `type: array` schema becomes a `pub type X = Vec<Item>;`
    /// alias (no longer dropped), reusing the referenced item type.
    #[test]
    fn top_level_array_alias() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Tag:
      type: object
      required: [name]
      properties:
        name:
          type: string
    TagArray:
      type: array
      items:
        $ref: "#/components/schemas/Tag"
"##;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub type TagArray = Vec<Tag>;"),
            "missing array alias: {model}"
        );
        assert!(
            crate_.diagnostics.is_empty(),
            "array schema should no longer be dropped: {:?}",
            crate_.diagnostics
        );
        Ok(())
    }

    /// An operation whose success response offers >1 media type generates a
    /// content-negotiated response enum — even in models-only mode.
    #[test]
    fn content_negotiated_response_enum() -> Result<()> {
        let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths:
  /members:
    get:
      operationId: listMembers
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                $ref: "#/components/schemas/MemberList"
            text/csv:
              schema:
                type: string
components:
  schemas:
    MemberList:
      type: object
      properties:
        size:
          type: integer
          format: int64
"##;
        // test_config() is model-only (server generation off).
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");
        assert!(
            model.contains("pub enum ListMembersResponse {")
                && model.contains("Json(MemberList),")
                && model.contains("Csv(String),"),
            "missing content-negotiated response enum: {model}"
        );
        Ok(())
    }

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
"#;
        let crate_ = generate(&load_spec(yaml)?, &test_config())?;
        let model = file_content(&crate_, "src/model.rs");
        assert!(model.contains("pub name: String,"));
        assert!(model.contains("pub count: i32,"));
        assert!(
            model.contains(r#"#[serde(default = "crate::default::default_config_name")]"#),
            "missing serde default attr for name: {model}"
        );
        let defaults = file_content(&crate_, "src/default.rs");
        assert!(defaults.contains("pub(crate) fn default_config_name() -> String"));
        assert!(defaults.contains(r#"String::from("default_name")"#));
        assert!(defaults.contains("pub(crate) fn default_config_count() -> i32"));
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(
            lib.contains("mod default;"),
            "lib.rs missing default: {lib}"
        );
        Ok(())
    }

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
        assert!(crate_.files.iter().all(|f| f.path != "src/default.rs"));
        let lib = file_content(&crate_, "src/lib.rs");
        assert!(
            !lib.contains("mod default;"),
            "no default module when no defaults: {lib}"
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

    /// Regression: an inline-enum `default` naming no variant used to emit a
    /// nonexistent enum variant (uncompilable). It is now dropped, so the
    /// generated crate compiles. Ignored by default (shells out to cargo check).
    #[test]
    #[ignore = "shells out to cargo check; run manually with --ignored"]
    fn bad_enum_default_compiles() -> Result<()> {
        use std::process::Command;

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
        status:
          type: string
          enum: [active, inactive]
          default: unknown
"#;
        let config = Config::new("bad_enum_default_compile_test".to_string(), false);
        let crate_ = generate(&load_spec(yaml)?, &config)?;

        // The bad default is reported and dropped.
        assert!(
            crate_
                .diagnostics
                .iter()
                .any(|d| d.construct == "enum default"),
            "expected an enum-default diagnostic: {:?}",
            crate_.diagnostics
        );

        let dir = std::env::temp_dir().join("openapi_modelgen_bad_enum_default_compile_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        for file in &crate_.files {
            std::fs::write(dir.join(file.path), &file.content)?;
        }

        let status = Command::new("cargo")
            .arg("check")
            .current_dir(&dir)
            .status()?;
        assert!(status.success(), "generated crate failed to compile");

        Ok(())
    }

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
            defaults.contains("pub(crate) fn default_foo_label() -> Option<String>"),
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
            defaults.contains("pub(crate) fn default_event_created_at() -> DateTime<Utc>"),
            "missing DateTime default fn: {defaults}"
        );
        assert!(
            defaults.contains(r#""2024-01-01T00:00:00Z".parse::<DateTime<Utc>>().expect("hardcoded default from OpenAPI spec")"#),
            "missing DateTime parse: {defaults}"
        );
        assert!(
            defaults.contains("pub(crate) fn default_event_event_date() -> NaiveDate"),
            "missing NaiveDate default fn: {defaults}"
        );
        assert!(
            defaults.contains(
                r#""2024-06-15".parse::<NaiveDate>().expect("hardcoded default from OpenAPI spec")"#
            ),
            "missing NaiveDate parse: {defaults}"
        );
        assert!(
            defaults.contains("pub(crate) fn default_event_event_id() -> Uuid"),
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
            defaults.contains("pub(crate) fn default_greeting_language() -> GreetingLanguage"),
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
            model.contains(
                r#"#[serde(default = "crate::default::default_get_things_query_limit")]"#
            ),
            "missing serde default attr: {model}"
        );

        let defaults = file_content(&crate_, "src/default.rs");
        assert!(
            defaults.contains("pub(crate) fn default_get_things_query_limit() -> i32"),
            "missing default fn for query param: {defaults}"
        );

        Ok(())
    }

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
      required: [name]
      properties:
        name:
          type: string
          minLength: 1
    Dog:
      type: object
      required: [name]
      properties:
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

    /// Compile-check the generated crate for a `oneOf` spec end-to-end.
    ///
    /// Ignored by default: it shells out to `cargo check` (needs a `cargo`
    /// toolchain and, on a cold cache, network access), which the fast unit
    /// suite deliberately avoids. Run explicitly with:
    /// `cargo test -- --ignored one_of_generated_crate_compiles`.
    #[test]
    #[ignore = "shells out to cargo check; run manually with --ignored"]
    fn one_of_generated_crate_compiles() -> Result<()> {
        use std::process::Command;

        // Fixed versions (not workspace refs) so the crate builds standalone.
        let config = Config::new("one_of_compile_test".to_string(), false);
        let crate_ = generate(&load_spec(ONE_OF_SPEC)?, &config)?;

        let dir = std::env::temp_dir().join("openapi_modelgen_one_of_compile_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        for file in &crate_.files {
            std::fs::write(dir.join(file.path), &file.content)?;
        }

        let status = Command::new("cargo")
            .arg("check")
            .current_dir(&dir)
            .status()?;
        assert!(status.success(), "generated crate failed to compile");

        Ok(())
    }

    /// Regression: a `pattern` with brace quantifiers used to emit an invalid
    /// `format!` string (braces read as placeholders) and fail to compile.
    /// Ignored by default — shells out to `cargo check`. Run with:
    /// `cargo test -- --ignored pattern_with_braces_compiles`.
    #[test]
    #[ignore = "shells out to cargo check; run manually with --ignored"]
    fn pattern_with_braces_compiles() -> Result<()> {
        use std::process::Command;

        let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    Contact:
      type: object
      required: [phone]
      properties:
        phone:
          type: string
          pattern: '^\+[1-9]\d{1,14}$'
"#;
        let config = Config::new("brace_pattern_compile_test".to_string(), false);
        let crate_ = generate(&load_spec(yaml)?, &config)?;

        let dir = std::env::temp_dir().join("openapi_modelgen_brace_pattern_compile_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))?;
        for file in &crate_.files {
            std::fs::write(dir.join(file.path), &file.content)?;
        }

        let status = Command::new("cargo")
            .arg("check")
            .current_dir(&dir)
            .status()?;
        assert!(status.success(), "generated crate failed to compile");

        Ok(())
    }
}
