//! Field naming: property names that are not Rust identifiers, the
//! camelCase to snake_case rewrite, and the collisions it can cause.

use openapi_modelgen::{generate, load_spec};

mod common;

use common::{Result, file_content, spec_error, test_config};

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
