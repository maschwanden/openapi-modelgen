//! Type naming: schema and operation names that are not Rust
//! identifiers, the collisions between them, and `$ref`s that resolve to nothing.

use openapi_modelgen::{generate, load_spec};
use pretty_assertions::assert_eq;

mod common;

use common::{Result, file_content, spec_error, test_config};

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
            r#"schema "foo-bar" and schema "foo_bar" would both become the Rust type `FooBar`"#
        ),
        "the error should name both schemas: {message}"
    );

    Ok(())
}

/// An `operationId` names a query struct the same way a schema name names a
/// struct, so two operations that share one collide. OpenAPI requires the id
/// to be unique, but nothing enforces it, and the output is two `pub struct
/// GetThingsQuery` items that do not compile.
#[test]
fn operations_sharing_an_operation_id_abort() -> Result<()> {
    let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths:
  /a:
    get:
      operationId: getThings
      parameters:
        - name: limit
          in: query
          schema:
            type: integer
      responses:
        "200":
          description: OK
  /b:
    get:
      operationId: getThings
      parameters:
        - name: offset
          in: query
          schema:
            type: integer
      responses:
        "200":
          description: OK
components:
  schemas: {}
"#;
    let message = spec_error(yaml);
    assert!(
        message.contains(
            "operation \"getThings\" (GET /a) and operation \"getThings\" (GET /b) \
                 would both become the Rust type `GetThingsQuery`"
        ),
        "the error should name both operations: {message}"
    );

    Ok(())
}

/// A query struct shares one namespace with the schemas, so a schema can
/// claim the name an operation needs.
#[test]
fn a_query_struct_colliding_with_a_schema_aborts() -> Result<()> {
    let yaml = r#"
openapi: "3.0.3"
info:
  title: Test
  version: "0.1.0"
paths:
  /a:
    get:
      operationId: getThings
      parameters:
        - name: limit
          in: query
          schema:
            type: integer
      responses:
        "200":
          description: OK
components:
  schemas:
    GetThingsQuery:
      type: object
      properties:
        a:
          type: string
"#;
    let message = spec_error(yaml);
    assert!(
        message.contains(
            "schema \"GetThingsQuery\" and operation \"getThings\" (GET /a) \
                 would both become the Rust type `GetThingsQuery`"
        ),
        "the error should name the schema and the operation: {message}"
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
        message
            .contains(r##"$ref "#/components/schemas/Nope" names a schema that does not exist"##),
        "the reference should be named in full: {message}"
    );

    Ok(())
}

/// A `$ref` to a schema that is in the spec but produced no type (an `allOf`,
/// whose members the generator drops) leaves the field naming a type nothing
/// defines, so it aborts as well. An array element carries its own `$ref`, so
/// holding the schema directly and holding a `Vec` of it are both reported.
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

/// A schema whose own name has nothing an identifier can be built from
/// (`"!!!"`) aborts the same way a property name or enum value with nothing to
/// build on does.
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
