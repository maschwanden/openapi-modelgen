//! Enum naming: spec values that are not Rust identifiers, variants
//! that collide, and the names inline enums take from their surroundings.

use openapi_modelgen::{Diagnostic, Severity, generate, load_spec};
use pretty_assertions::assert_eq;

mod common;

use common::{Result, file_content, spec_error, test_config};

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

/// An inline enum has no name in the spec: the generator composes one from
/// the struct and the property. When a schema already holds that name, the
/// schema keeps it (it is the one the user can rename) and the inline enum
/// takes the `Inline` suffix. The `default` is what proves the new name
/// reached every writer, not just `model.rs`.
#[test]
fn an_inline_enum_takes_the_suffix_around_a_schema() -> Result<()> {
    let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    GreetingLanguage:
      type: object
      properties:
        code:
          type: string
    Greeting:
      type: object
      properties:
        language:
          type: string
          enum: [en, de]
          default: en
"##;
    let crate_ = generate(&load_spec(yaml)?, &test_config())?;

    let model = file_content(&crate_, "src/model.rs");
    assert!(
        model.contains(
            "\
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GreetingLanguageInline {
    #[serde(rename = \"en\")]
    En,
    #[serde(rename = \"de\")]
    De,
}
"
        ),
        "the inline enum should take the suffix: {model}"
    );
    assert!(
        model.contains(
            "\
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GreetingLanguage {
    pub code: Option<String>,
}
"
        ),
        "the schema should keep its own name: {model}"
    );
    assert!(
        model.contains("pub language: GreetingLanguageInline,"),
        "the field should name the suffixed enum: {model}"
    );

    let defaults = file_content(&crate_, "src/default.rs");
    assert!(
        defaults.contains(
            "\
pub fn greeting_language() -> GreetingLanguageInline {
    GreetingLanguageInline::En
}
"
        ),
        "the default fn should name the suffixed enum: {defaults}"
    );

    let validation = file_content(&crate_, "src/validation.rs");
    assert!(
        validation.contains("impl Validation for GreetingLanguageInline {}"),
        "the Validation impl should name the suffixed enum: {validation}"
    );

    let renamed: Vec<&Diagnostic> = crate_
        .diagnostics
        .iter()
        .filter(|d| d.construct == "inline enum")
        .collect();
    assert_eq!(renamed.len(), 1, "{:?}", crate_.diagnostics);
    assert_eq!(renamed[0].severity, Severity::Degraded);
    assert_eq!(renamed[0].path, "Greeting.language");
    assert_eq!(
        renamed[0].reason,
        "`GreetingLanguage` is taken by another type; \
             the inline enum was named `GreetingLanguageInline` instead"
    );

    Ok(())
}

/// Two inline enums can want one name too, since the struct and the
/// property run together (`Foo` + `barBaz` and `FooBar` + `baz` both give
/// `FooBarBaz`). The suffix then marks whichever asked second.
#[test]
fn two_inline_enums_that_want_one_name() -> Result<()> {
    let yaml = r##"
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
        barBaz:
          type: string
          enum: [a, b]
    FooBar:
      type: object
      properties:
        baz:
          type: string
          enum: [c, d]
"##;
    let crate_ = generate(&load_spec(yaml)?, &test_config())?;

    let model = file_content(&crate_, "src/model.rs");
    assert!(
        model.contains("pub enum FooBarBaz {"),
        "the first inline enum keeps the composed name: {model}"
    );
    assert!(
        model.contains("pub enum FooBarBazInline {"),
        "the second takes the suffix: {model}"
    );
    assert!(model.contains("pub bar_baz: Option<FooBarBaz>,"), "{model}");
    assert!(
        model.contains("pub baz: Option<FooBarBazInline>,"),
        "{model}"
    );

    let renamed: Vec<&Diagnostic> = crate_
        .diagnostics
        .iter()
        .filter(|d| d.construct == "inline enum")
        .collect();
    assert_eq!(renamed.len(), 1, "{:?}", crate_.diagnostics);
    assert_eq!(renamed[0].path, "FooBar.baz");

    Ok(())
}

/// One suffix deep only. With both names taken the generator has nothing
/// left to derive from, and a third invented name would appear nowhere in
/// the spec, so the clash is fatal like any other.
#[test]
fn an_inline_enum_with_both_names_taken_aborts() -> Result<()> {
    let yaml = r##"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths: {}
components:
  schemas:
    GreetingLanguage:
      type: object
      properties:
        code:
          type: string
    GreetingLanguageInline:
      type: object
      properties:
        code:
          type: string
    Greeting:
      type: object
      properties:
        language:
          type: string
          enum: [en, de]
"##;
    let message = spec_error(yaml);
    assert!(
        message.contains(
            "`GreetingLanguage` and `GreetingLanguageInline` are both taken; \
                 rename the schema or the property"
        ),
        "the error should name both candidates: {message}"
    );

    Ok(())
}
