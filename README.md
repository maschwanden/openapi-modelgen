# openapi-modelgen

Generate Rust types from an OpenAPI 3.0 spec. The generated crate gives you plain structs with serde support and runtime validation.

## What it generates

Given an OpenAPI spec, `openapi-modelgen` produces a self-contained Rust crate with:

- **`Cargo.toml`**: package manifest with all required dependencies
- **`src/lib.rs`**: module declarations and re-exports
- **`src/model.rs`**: Rust structs derived from `components/schemas` (with `Serialize` + `Deserialize`) and query parameter structs from path operations (with `Deserialize`)
- **`src/validation.rs`**: a `Validation` trait with `validate()` implementations that enforce OpenAPI constraints at runtime

### Supported features

| OpenAPI construct | Generated Rust code |
|---|---|
| `components/schemas` (object) | `struct` with `Serialize + Deserialize` |
| Query parameters (`in: query`) | `struct` with `Deserialize` only |
| `$ref` to other schemas | Nested struct field |
| `type: string` | `String` (or `DateTime<Utc>` / `NaiveDate` for date formats) |
| `type: integer` (int32/int64) | `i32` / `i64` |
| `type: number` | `f64` |
| `type: boolean` | `bool` |
| `type: array` | `Vec<T>` |
| Inline `enum` (string) | Rust `enum` with `#[serde(rename)]` variants |
| `oneOf` (top-level, `$ref` members) | Rust `enum`; `#[serde(tag)]` when a `discriminator` is present, `#[serde(untagged)]` otherwise |
| `nullable` / not `required` | `Option<T>` |

### Validation constraints

The generated `validate()` methods enforce:

- `minLength` / `maxLength` / `pattern` / `enum` for strings
- `minimum` / `maximum` / `exclusiveMinimum` / `exclusiveMaximum` / `multipleOf` / `enum` for integers
- `minimum` / `maximum` / `exclusiveMinimum` / `exclusiveMaximum` / `multipleOf` for numbers
- `minItems` / `maxItems` / `uniqueItems` for arrays
- Recursive validation for nested `$ref` objects and `Vec<$ref>`

Validation errors include field paths for nested structs (e.g. `child.name: length 0 is less than minimum 1`).

### Discriminated (`oneOf` + `discriminator`) unions

A top-level `oneOf` with a `discriminator` becomes an internally tagged enum, and `mapping` entries set each variant's wire value. Without a `mapping`, OpenAPI implies the member's schema name, which is used instead.

Because `#[serde(tag = "p")]` makes serde emit and consume the `p` key itself, **the discriminator property is removed from the member structs**. Specs normally declare it on every member; leaving it in place would serialize the key twice and fail to deserialize with `missing field p`. The key is still read and written on the wire — it just lives on the enum rather than the struct.

A member reached *directly* (a field typed with it rather than with the union) therefore has no `p` key: nothing re-adds it, and nothing needs to, since the field's static type already says which member it is. Reaching for one arm of a discriminator instead of the union is usually a sign the field should have been typed with the union.

Members of a discriminated union must be `$ref`s to local object schemas. A member that names a non-object schema is dropped, because serde's internally tagged representation requires each payload to serialize as a map.

### Naming

Schema names, property names and enum values are arbitrary spec strings, and most of them are not legal Rust identifiers. They are converted to the Rust casing of their kind (`PascalCase` types and variants, `snake_case` fields, so the generated crate is free of `non_snake_case` and `non_camel_case_types` warnings) and sanitized before being emitted. The original string is put back with a `#[serde(rename)]`, so the wire format is unchanged:

| Spec | Generated |
| --- | --- |
| enum value `10min` | variant `Variant10min` (a leading digit cannot open an identifier) |
| enum value `with space`, `a.b` | variant `WithSpace`, `AB` |
| property `firstName`, `first-name` | field `first_name` |
| property `HTTPProxyURL` | field `http_proxy_url` |
| property `type` | field `r#type` (no rename needed, serde strips the `r#`) |
| property `self` | field `self_` (`self` cannot be a raw identifier) |
| schema `3d-model` | type `Type3dModel` |

Sanitizing loses nothing (the value is still on the wire), so it is not reported.

Two naming cases are fatal:

- A spec name with **nothing to build on** (the enum value `""`, a property or schema named `"!!!"` or `"_"`) leaves nothing to derive from.
- A name **collision**: Two enum values can sanitize to the same variant (`a.b` and `a-b` both give `AB`, and so do `10min` and a literal `Variant10min`), as can two properties of one struct (`first-name` and `first.name`) or two schema names (`foo-bar` and `foo_bar`).

```
error: code generation failed: 3 fatal problems in the spec

  Series.odd
    enum values "a.b" and "a-b" would both become the Rust enum variant `AB`

  Series.resolution
    enum values "10min" and "Variant10min" would both become the Rust enum variant `Variant10min`

  Series.kind
    enum value "" has nothing a Rust name can be built from

Fix the spec, then re-run. No files were written.
```

A **`$ref` that resolves to no type** is fatal for the same reason, one step removed: the field would name a Rust type that nothing defines. That covers a `$ref` to a schema that is not in the spec, and a `$ref` to one that generated nothing (an `allOf` schema, or a `oneOf` whose members were all dropped). External `$ref`s are excluded, since they are unsupported by design and already carry their own diagnostic.

### Limitations / not yet supported

- **Inline / field-level `oneOf`**: a `oneOf` used directly as a property's schema (rather than a named schema under `components/schemas`) is not generated as a typed enum — the field falls back to `serde_json::Value`.
- **Non-`$ref` members of a `oneOf`**: inline-object members of a top-level `oneOf` are skipped; only `$ref`s to local schemas become variants. Members whose target schema was itself not generated, or whose name collides with another variant, are dropped too.
- **`anyOf` / `allOf`**: not supported. Schemas using them are dropped (top-level) or degrade to `serde_json::Value` (as an inline field schema). A `$ref` *to* such a schema is fatal, see [Naming](#naming).
- **Untagged union cardinality is not enforced**: an untagged `oneOf` (no discriminator) deserializes to the first matching variant; the "exactly one match" rule is not validated at runtime.
- **Request/response bodies, `additionalProperties` (maps), header/cookie parameters, non-local `$ref`s, and non-object/non-string-enum top-level schemas** are not generated.

None of these are silent: every construct that is dropped or degraded is reported as a **diagnostic**. The CLI prints a summary to stderr after generation, and library callers get the full list in `GeneratedCrate.diagnostics`. A *fatal* diagnostic (see [Naming](#naming)) is different in kind: it aborts generation, so it arrives as an error rather than in that list.

Pass `--strict` to make the CLI exit non-zero (code 2) when *anything* was not fully generated — degraded as well as dropped. Degrading still loses information, and some degradations (an external `$ref`, say) leave code that will not compile, so a CI gate for lossless generation has to treat both as failures. Files are always written; only the exit code changes.

## Installation

### From GitHub (with Cargo)

Requires Rust 1.85+ (edition 2024).

```sh
cargo install --git https://github.com/maschwanden/openapi-modelgen.git
```

### From a local clone

```sh
git clone https://github.com/maschwanden/openapi-modelgen.git
cd openapi-modelgen
cargo install --path .
```

### With Nix

Run directly without installing:

```sh
nix run github:maschwanden/openapi-modelgen -- \
  --input spec.yaml --output-dir ./generated --crate-name my-api
```

Or install into your profile:

```sh
nix profile install github:maschwanden/openapi-modelgen
```

From a local checkout, `nix build` produces the binary in `./result/bin/openapi-modelgen`.

## Usage

```sh
openapi-modelgen \
  --input path/to/openapi.yaml \
  --output-dir ./generated \
  --crate-name my-api-models
```

This creates the crate at `./generated/my-api-models/`:

```
generated/
└── my-api-models/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        ├── model.rs
        └── validation.rs
```

### Options

| Flag | Description |
|---|---|
| `--input <PATH>` | Path to the OpenAPI 3.0 YAML spec |
| `--output-dir <DIR>` | Parent directory — the crate is written to `<DIR>/<crate-name>/` |
| `--crate-name <NAME>` | Cargo package name for the generated crate (also used as the subdirectory name) |
| `--workspace` | Use `foo.workspace = true` style dependency references instead of fixed versions |

### Examples

Given [this OpenAPI spec](examples/axum-hello-world/openapi.yaml), `openapi-modelgen` generates types like these:

```rust
pub struct Greeting {
    pub id: i64,
    pub message: String,                // minLength: 1, maxLength: 280
    pub language: GreetingLanguage,      // enum: en, de, fr
    pub created_at: DateTime<Utc>,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,       // maxItems: 10, uniqueItems
}

pub struct CreateGreetingRequest {
    pub message: String,
    pub language: GreetingLanguage,
    pub expires_on: Option<NaiveDate>,
    pub tags: Option<Vec<String>>,
}
```

You use them directly in your handlers, e.g. with axum:

```rust
use hello_world_openapi::{CreateGreetingRequest, Greeting, Validation};

async fn create_greeting(
    Json(body): Json<CreateGreetingRequest>,
) -> Result<(StatusCode, Json<Greeting>), (StatusCode, String)> {
    if let Err(e) = body.validate() {
        return Err((StatusCode::BAD_REQUEST, e.to_string()));
    }

    let greeting = Greeting {
        id: 1,
        message: body.message,
        language: body.language,
        created_at: Utc::now(),
        expires_on: body.expires_on,
        tags: body.tags,
    };
    Ok((StatusCode::CREATED, Json(greeting)))
}
```

Full working examples:

- [`examples/axum-with-custom-extractors`](examples/axum-with-custom-extractors): Basic axum workspace using generated types in handlers with custom `JsonV`/`QueryV` extractors that validate automatically (handlers never call `.validate()`)

## Development

```sh
# Enter dev shell (provides Rust toolchain, cargo, clippy, rustfmt, rust-analyzer)
nix develop

# Build
cargo build

# Run tests
cargo test

# Format
cargo fmt

# Lint
cargo clippy
```
