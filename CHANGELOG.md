# Changelog

This document describes notable changes to `openapi-modelgen`, the tool that
generates a self-contained Rust crate (models, runtime validation, and an
optional server layer) from an OpenAPI 3.0 spec.

## v0.1.13 (2026-07-27)

### Added

- Content-negotiated responses. An operation whose success response declares
  more than one media type now generates a `{Operation}Response` enum (e.g.
  `enum ListMembersResponse { Json(MemberList), Csv(String) }`) — a model-layer
  type, so it is available in models-only mode too. The `Api` trait method
  returns it, and the axum adapter renders each variant with its own
  `Content-Type`. Negotiation is impl-owned and CSV/binary bodies stay
  hand-written; only the route and the JSON model are compile-enforced. This
  lets CSV-export endpoints be `strict` instead of hand-wired `manual`.

### Fixed

- A single non-JSON success body (a pure `text/csv` or `application/octet-stream`
  download) is now returned with its `Content-Type` instead of regressing to an
  empty `200`.

## v0.1.12 (2026-07-26)

### Added

- Top-level `type: array` schemas are now generated as a type alias
  (`pub type TagArray = Vec<Tag>;`) instead of being dropped, reusing the
  referenced item type. Server operations that `$ref` such a schema get a typed
  response.

### Fixed

- The "inline body not generated as a named type" diagnostic now inspects only
  the `application/json` content. A response whose JSON body is a `$ref` but
  that also offers a non-JSON alternate (`text/csv`, `text/html`) is no longer
  falsely reported as an ungenerated body — so `--strict` is not blocked by CSV
  export endpoints whose JSON is already a named schema.

## v0.1.11 (2026-07-26)

### Fixed

- Server generation: honor the operation's declared success status code — a
  `201 Created` or `204 No Content` response is emitted instead of always
  returning `200`.
- Server generation: type path parameters from the spec (`integer/int64` →
  `i64`, `int32` → `i32`, `number` → `f64`, `boolean` → `bool`) instead of
  always `String`. This changes the generated `Api` trait method signatures, so
  existing implementations must adopt the typed parameter.
- Server generation: extract query parameters via `axum-extra`, so repeated
  keys (`?k=a&k=b`) collect into a `Vec` instead of collapsing to a single
  value. The generated `Cargo.toml` gains an `axum-extra` dependency (behind the
  `server-axum` feature) when a query extractor is emitted.
- Server generation: derive unique, valid handler and query-struct names from
  the path when an operation has no `operationId`. Sibling paths like `/things`
  and `/things/{id}` no longer collide into the same trait method, and segments
  with characters such as `.` (e.g. `/spec/openapi.yaml`) no longer produce
  invalid Rust identifiers. Names that still collide (e.g. duplicate
  operationIds) are reported as a diagnostic instead of silently emitting a
  crate that will not compile.

## v0.1.10 (2026-07-26)

### Added

- Server generation: emit a framework-agnostic `Api` trait from path
  operations, plus an opt-in axum adapter (`server::router`). Selected via an
  `openapi-modelgen.yaml` config file (`generate: { model, server, axum-server }`,
  `server-style`, per-operation overrides) or the `--server` CLI shorthand.

## v0.1.9 (2026-07-22)

### Added

- Top-level `oneOf` schema support: generated as serde unions — internally
  tagged (`#[serde(tag = "…")]`) when a `discriminator` is present (honoring its
  `mapping`), untagged otherwise. Each union gets a `Validation` impl that
  delegates to the active variant.
- Diagnostics: every spec construct that cannot be fully represented (dropped or
  degraded) is now reported. `generate()` returns the list to library callers,
  and the CLI prints a summary. A new `--strict` flag makes the CLI exit
  non-zero when any construct was dropped, so CI can gate on lossless generation.

### Fixed

- Escape braces in generated pattern-mismatch error messages so patterns with
  brace quantifiers (e.g. `{1,14}`) no longer emit an invalid `format!` string.
- Validate inline-enum default values and drop invalid ones instead of emitting
  a nonexistent (uncompilable) enum variant.
- Diagnose request/response bodies by their inline content, resolving `$ref`
  bodies and responses first so a `$ref` to a generated schema is not falsely
  reported as a loss.
- Render each field default exactly once, shared between `model.rs` and
  `default.rs`, and stop double-logging diagnostics.

## v0.1.8 (2026-06-02)

### Added

- Parse OpenAPI `default` values and honor them in code generation: fields with
  a supported default are emitted with `#[serde(default = "…")]`, and a
  `src/default.rs` module provides the default functions.

## v0.1.7 (2026-06-01)

### Added

- Generate `Validation` impls for inline enums.

### Changed

- Only import the chrono types actually used by the generated structs, avoiding
  unused-import warnings.

## v0.1.6 (2026-05-28)

### Added

- Derive query struct names from the HTTP method and path when an operation has
  no `operationId`.

## v0.1.5 (2026-05-28)

### Added

- Generate standalone enums from top-level `type: string` enum schemas.

## v0.1.4 (2026-05-24)

### Fixed

- Silence potential clippy lints in the generated code.

## v0.1.3 (2026-05-24)

### Added

- Add a `--version` flag to the CLI.

## v0.1.2 (2026-05-24)

### Fixed

- Address several potential linting and formatting problems in the generated
  code.

## v0.1.1 (2026-05-21)

### Fixed

- Escape Rust keywords when used as field identifiers (e.g. `type` → `r#type`).

## v0.1.0 (2026-04-25)

Initial release.

- Generate a self-contained Rust crate from an OpenAPI 3.0 spec: `Cargo.toml`,
  `src/lib.rs`, `src/model.rs` (structs from `components/schemas` and query
  parameters, with serde derives), and `src/validation.rs` (a `Validation` trait
  enforcing OpenAPI constraints at runtime).
