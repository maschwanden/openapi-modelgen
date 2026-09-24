//! Structural end-to-end generation: what a spec turns into on disk,
//! and the shape of the types inside each generated file.

use openapi_modelgen::{generate, load_spec};
use pretty_assertions::assert_eq;

mod common;

use common::{Result, file_content, test_config};

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
