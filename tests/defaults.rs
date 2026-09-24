//! Generation of `default` values: which ones survive into a `Default`
//! impl, how they are rendered, and when the default module is skipped.

use openapi_modelgen::{generate, load_spec};
use pretty_assertions::assert_eq;

mod common;

use common::{Result, file_content, spec_error, test_config};

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
