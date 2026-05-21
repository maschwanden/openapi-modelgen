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
| `nullable` / not `required` | `Option<T>` |

### Validation constraints

The generated `validate()` methods enforce:

- `minLength` / `maxLength` / `pattern` / `enum` for strings
- `minimum` / `maximum` / `exclusiveMinimum` / `exclusiveMaximum` / `multipleOf` / `enum` for integers
- `minimum` / `maximum` / `exclusiveMinimum` / `exclusiveMaximum` / `multipleOf` for numbers
- `minItems` / `maxItems` / `uniqueItems` for arrays
- Recursive validation for nested `$ref` objects and `Vec<$ref>`

Validation errors include field paths for nested structs (e.g. `child.name: length 0 is less than minimum 1`).

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
