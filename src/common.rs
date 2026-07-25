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

/// Derive a PascalCase name from an HTTP method and path.
///
/// E.g. `("get", "/api/value-raw/{id}/timeseries")` → `"GetValueRawTimeseries"`.
/// Path parameters like `{id}` are stripped.
pub(crate) fn query_name_from_path(method: &str, path: &str) -> String {
    let segments = path
        .split('/')
        .filter(|s| !s.is_empty() && !s.starts_with('{'));
    let mut name = to_pascal_case(method);
    for segment in segments {
        name.push_str(&to_pascal_case(segment));
    }
    name
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

use openapiv3::ReferenceOr;
