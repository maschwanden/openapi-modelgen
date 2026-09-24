//! What `generate` reports about constructs it could not fully
//! represent, and what it stays silent about.

use openapi_modelgen::{Severity, generate, load_spec};

mod common;

use common::{Result, file_content, test_config};

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
