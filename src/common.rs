//! Helpers shared between model generation and server generation.

/// Rust strict keywords that must be escaped with `r#` when used as identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

/// The standard header comment prepended to every generated file.
pub(crate) fn header_comment() -> &'static str {
    "This file is @generated — do not edit manually."
}

/// Escape a name with `r#` if it is a Rust keyword.
pub(crate) fn escape_keyword(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// Convert a `PascalCase` string to `snake_case`.
pub(crate) fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.extend(c.to_lowercase());
    }
    result
}

/// Convert a `snake_case` or `kebab-case` string to `PascalCase`.
pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if c == '_' || c == '-' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

/// Derive a unique, valid PascalCase identifier from an HTTP method and path.
///
/// Used when an operation has no `operationId`. Every path segment contributes,
/// including path parameters (as `By{Name}`), so sibling paths like `/things`
/// and `/things/{id}` yield distinct names (`GetThings` vs `GetThingsById`)
/// instead of colliding into the same trait method / query struct. Any
/// non-alphanumeric character in a segment (e.g. the `.` in `openapi.yaml`) is
/// treated as a word boundary so the result is always a valid Rust identifier.
///
/// E.g. `("get", "/api/value-raw/{id}/timeseries")` → `"GetApiValueRawByIdTimeseries"`.
pub(crate) fn query_name_from_path(method: &str, path: &str) -> String {
    let mut name = pascal_segment(method);
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        match segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            Some(param) => {
                name.push_str("By");
                name.push_str(&pascal_segment(param));
            }
            None => name.push_str(&pascal_segment(segment)),
        }
    }
    name
}

/// PascalCase a single path/identifier fragment, treating every non-alphanumeric
/// character as a word boundary (so `openapi.yaml` → `OpenapiYaml`, `value-raw`
/// → `ValueRaw`). Keeps the derived name a valid Rust identifier fragment.
fn pascal_segment(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    to_pascal_case(&sanitized)
}

/// Extract the schema name from a `$ref` string (e.g. `#/components/schemas/Foo` -> `Foo`).
pub(crate) fn resolve_ref_name(reference: &str) -> String {
    reference
        .strip_prefix("#/components/schemas/")
        .unwrap_or(reference)
        .to_string()
}

/// Extract the common `ParameterData` from any parameter variant (query, header, path, cookie).
pub(crate) fn parameter_data(param: &openapiv3::Parameter) -> &openapiv3::ParameterData {
    match param {
        openapiv3::Parameter::Query { parameter_data, .. }
        | openapiv3::Parameter::Header { parameter_data, .. }
        | openapiv3::Parameter::Path { parameter_data, .. }
        | openapiv3::Parameter::Cookie { parameter_data, .. } => parameter_data,
    }
}

/// Resolve a `$ref` string like `#/components/parameters/Foo` to the actual parameter.
pub(crate) fn resolve_parameter_ref<'a>(
    reference: &str,
    components: Option<&'a openapiv3::Components>,
) -> Option<&'a openapiv3::Parameter> {
    let name = reference.strip_prefix("#/components/parameters/")?;
    let params = &components.as_ref()?.parameters;
    match params.get(name)? {
        ReferenceOr::Item(p) => Some(p),
        ReferenceOr::Reference { .. } => None,
    }
}

use openapiv3::{MediaType, Operation, ReferenceOr, Responses, SchemaKind, StatusCode, Type};

/// The PascalCase base name for an operation-derived type: the `operationId`
/// (normalised) if present, else derived from the method + path. Callers append
/// the role suffix (`Query`, `Response`).
pub(crate) fn operation_base_name(op: &Operation, method: &str, path: &str) -> String {
    match &op.operation_id {
        Some(id) => to_pascal_case(id),
        None => query_name_from_path(method, path),
    }
}

/// One representable body of a success response, keyed by media type.
pub(crate) struct ResponseMedia {
    /// Enum variant name (`Json`, `Csv`, `Html`, `Text`, `Binary`).
    pub variant: String,
    /// The MIME type, e.g. `application/json`, `text/csv`.
    pub media_type: String,
    /// Model-local Rust type (e.g. `MemberList`, `Vec<Member>`, `String`).
    pub model_type: String,
    /// Crate-prefixed Rust type (e.g. `crate::MemberList`, `Vec<crate::Member>`).
    pub crate_type: String,
    /// Whether this body is JSON (rendered via `axum::Json`).
    pub is_json: bool,
}

/// The success (2xx) response's status code and its representable media bodies.
///
/// Prefers the first 2xx response that carries at least one representable body;
/// otherwise the first 2xx (e.g. a `204` with no content). Defaults to `200`
/// with no body when the operation declares no 2xx response. Only
/// `application/json` (via `$ref`), `text/*` and `application/octet-stream`
/// bodies are represented; anything else is skipped.
pub(crate) fn success_response_media(responses: &Responses) -> (u16, Vec<ResponseMedia>) {
    let mut chosen: Option<(u16, Vec<ResponseMedia>)> = None;
    for (code, resp_ref) in &responses.responses {
        let StatusCode::Code(code) = code else {
            continue;
        };
        if !(200..300).contains(code) {
            continue;
        }
        let media = match resp_ref {
            ReferenceOr::Item(resp) => resp
                .content
                .iter()
                .filter_map(|(mt, media)| classify_media(mt, media))
                .collect(),
            ReferenceOr::Reference { .. } => Vec::new(),
        };
        match &chosen {
            None => chosen = Some((*code, media)),
            Some((_, existing)) if existing.is_empty() && !media.is_empty() => {
                chosen = Some((*code, media));
            }
            _ => {}
        }
    }
    chosen.unwrap_or((200, Vec::new()))
}

/// Classify one response media type into a [`ResponseMedia`], or `None` if it is
/// not representable (e.g. an inline JSON object, or an unknown media type).
fn classify_media(media_type: &str, media: &MediaType) -> Option<ResponseMedia> {
    if media_type == "application/json" {
        let (name, array) = json_model_type(media.schema.as_ref())?;
        let (model_type, crate_type) = if array {
            (format!("Vec<{name}>"), format!("Vec<crate::{name}>"))
        } else {
            (name.clone(), format!("crate::{name}"))
        };
        return Some(ResponseMedia {
            variant: "Json".to_string(),
            media_type: media_type.to_string(),
            model_type,
            crate_type,
            is_json: true,
        });
    }
    let (variant, ty) = if media_type == "application/octet-stream" {
        ("Binary", "Vec<u8>")
    } else if let Some(sub) = media_type.strip_prefix("text/") {
        match sub {
            "csv" => ("Csv", "String"),
            "html" => ("Html", "String"),
            _ => ("Text", "String"),
        }
    } else {
        return None;
    };
    Some(ResponseMedia {
        variant: variant.to_string(),
        media_type: media_type.to_string(),
        model_type: ty.to_string(),
        crate_type: ty.to_string(),
        is_json: false,
    })
}

/// Resolve a JSON response schema to `(model_name, is_array)`. A direct `$ref`
/// yields `(name, false)`; an array whose `items` is a `$ref` yields
/// `(name, true)`. Inline schemas are not representable → `None`.
fn json_model_type(schema: Option<&ReferenceOr<openapiv3::Schema>>) -> Option<(String, bool)> {
    match schema? {
        ReferenceOr::Reference { reference } => Some((resolve_ref_name(reference), false)),
        ReferenceOr::Item(schema) => {
            if let SchemaKind::Type(Type::Array(arr)) = &schema.schema_kind
                && let Some(ReferenceOr::Reference { reference }) = &arr.items
            {
                return Some((resolve_ref_name(reference), true));
            }
            None
        }
    }
}
