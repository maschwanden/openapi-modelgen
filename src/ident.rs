//! Construction of Rust identifiers from spec strings.
//!
//! Every identifier the generator emits (type names, enum variants, struct
//! fields, default functions) is derived from a string in the OpenAPI spec,
//! and a spec is under no obligation to look like Rust: `10min`, `a.b`,
//! `with space` and `""` are all legal enum values, and property and schema
//! names are just as free. This module is the single place that turns such a
//! string into something that compiles.

/// Rust strict keywords that must be escaped with `r#` when used as identifiers.
const RUST_KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while",
];

/// Keywords that cannot be written as raw identifiers at all (RFC 2151), so
/// `r#` cannot rescue them. They get a trailing underscore instead.
const UNRAWABLE_KEYWORDS: &[&str] = &["crate", "self", "Self", "super"];

/// Prefix for an enum value that cannot open an identifier on its own, e.g. the
/// value `10min` → `Variant10min`.
const VARIANT_PREFIX: &str = "Variant";

/// Prefix for a type name that cannot open an identifier on its own, e.g. the
/// schema `3d-model` → `Type3dModel`.
const TYPE_PREFIX: &str = "Type";

/// Convert a `snake_case`, `kebab-case` or otherwise punctuated string to
/// `PascalCase`.
///
/// Every non-alphanumeric character is a word separator and is dropped, so
/// `a.b` and `a-b` both become `AB`. Casing inside a word is preserved:
/// `v1alpha` becomes `V1alpha`, not `V1Alpha`.
pub(crate) fn to_pascal_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if !c.is_alphanumeric() {
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

/// Convert an arbitrary string to `snake_case`.
///
/// Words break on punctuation and on case: `firstName` and `first-name` both
/// give `first_name`. An acronym is one word up to the letter that starts the
/// next one, so `HTTPProxyURL` gives `http_proxy_url` rather than
/// `h_t_t_p_proxy_u_r_l`.
///
/// The result can be empty, or start with a digit; [`to_field_ident`] is what
/// turns it into an identifier.
pub(crate) fn to_snake_case(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len() + 4);
    let mut pending_separator = false;

    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            pending_separator = !result.is_empty();
            continue;
        }
        let starts_word = match i.checked_sub(1).map(|prev| chars[prev]) {
            // `aB`, `1B`: a lowercase or digit followed by uppercase.
            Some(prev) if prev.is_alphanumeric() && !prev.is_uppercase() => c.is_uppercase(),
            // `HTTPProxy`: the last uppercase of a run starts the next word.
            Some(prev) if prev.is_uppercase() => {
                c.is_uppercase() && chars.get(i + 1).is_some_and(|next| next.is_lowercase())
            }
            _ => false,
        };
        if (pending_separator || starts_word) && !result.is_empty() {
            result.push('_');
        }
        pending_separator = false;
        result.extend(c.to_lowercase());
    }

    result
}

/// Turn an arbitrary spec string into a valid Rust type identifier.
pub(crate) fn to_type_ident(s: &str) -> Option<String> {
    let mut ident = to_pascal_case(s);
    if ident.is_empty() {
        return None;
    }
    if !starts_ident(&ident) {
        ident.insert_str(0, TYPE_PREFIX);
    }
    if UNRAWABLE_KEYWORDS.contains(&ident.as_str()) {
        ident.push('_');
    }
    Some(ident)
}

/// Turn an enum value into a valid Rust variant identifier.
pub(crate) fn to_variant_ident(s: &str) -> Option<String> {
    let mut ident = to_pascal_case(s);
    if ident.is_empty() {
        return None;
    }
    if !starts_ident(&ident) {
        ident.insert_str(0, VARIANT_PREFIX);
    }
    if UNRAWABLE_KEYWORDS.contains(&ident.as_str()) {
        ident.push('_');
    }
    Some(ident)
}

/// Turn a spec property name into a valid Rust field identifier.
pub(crate) fn to_field_ident(s: &str) -> Option<String> {
    let mut ident = to_snake_case(s);
    if ident.is_empty() {
        return None;
    }
    if !starts_ident(&ident) {
        ident.insert(0, '_');
    }
    if UNRAWABLE_KEYWORDS.contains(&ident.as_str()) {
        ident.push('_');
    }
    Some(ident)
}

/// Escape a keyword so it can be used as an identifier.
pub(crate) fn escape_keyword(name: &str) -> String {
    if RUST_KEYWORDS.contains(&name) {
        format!("r#{name}")
    } else {
        name.to_string()
    }
}

/// Whether `s` can be written as a Rust identifier (keywords aside).
pub(crate) fn is_valid_ident(s: &str) -> bool {
    if s == "_" {
        // A lone underscore is the wildcard pattern, not a usable name.
        return false;
    }
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    starts_ident_char(first) && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Assert that an identifier about to be emitted is valid Rust, and return it.
///
/// Every name reaching a writer has been through [`to_type_ident`] or
/// [`to_field_ident`], so a failure here is a generator bug, one that would
/// otherwise surface as uncompilable generated code with no explanation.
pub(crate) fn assert_ident(s: &str) -> &str {
    assert!(
        is_valid_ident(s),
        "generator bug: `{s}` is not a valid Rust identifier; \
         every emitted name must come from ident::to_type_ident or ident::to_field_ident"
    );
    s
}

fn starts_ident(s: &str) -> bool {
    s.chars().next().is_some_and(starts_ident_char)
}

fn starts_ident_char(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn pascal_case_splits_on_any_punctuation() {
        assert_eq!(to_pascal_case("charging_frequency"), "ChargingFrequency");
        assert_eq!(to_pascal_case("kebab-case"), "KebabCase");
        assert_eq!(to_pascal_case("a.b"), "AB");
        assert_eq!(to_pascal_case("with space"), "WithSpace");
        assert_eq!(to_pascal_case("x/y"), "XY");
        assert_eq!(to_pascal_case("+plus"), "Plus");
    }

    /// Casing inside a word is preserved, so existing names keep the spelling
    /// callers already depend on.
    #[test]
    fn pascal_case_preserves_inner_casing() {
        assert_eq!(to_pascal_case("v1alpha"), "V1alpha");
        assert_eq!(to_pascal_case("PascalCase"), "PascalCase");
    }

    #[test]
    fn type_ident_prefixes_a_leading_digit() {
        assert_eq!(to_type_ident("3d-model").as_deref(), Some("Type3dModel"));
        assert_eq!(to_type_ident("10min").as_deref(), Some("Type10min"));
    }

    #[test]
    fn variant_ident_prefixes_a_leading_digit() {
        assert_eq!(to_variant_ident("10min").as_deref(), Some("Variant10min"));
        assert_eq!(to_variant_ident("1h").as_deref(), Some("Variant1h"));
        assert_eq!(to_variant_ident("1").as_deref(), Some("Variant1"));
        assert_eq!(to_variant_ident("P1D").as_deref(), Some("P1D"));
        assert_eq!(to_variant_ident("hourly").as_deref(), Some("Hourly"));
    }

    /// The prefix is a plain string, so a spec can contain a value that already
    /// spells the prefixed name. Both are produced here; the caller is what
    /// resolves the clash.
    #[test]
    fn variant_ident_can_collide_with_a_spelled_out_value() {
        assert_eq!(to_variant_ident("10min"), to_variant_ident("Variant10min"));
    }

    #[test]
    fn type_ident_handles_keywords() {
        assert_eq!(to_type_ident("self").as_deref(), Some("Self_"));
        assert_eq!(to_type_ident("crate").as_deref(), Some("Crate"));
        assert_eq!(to_variant_ident("self").as_deref(), Some("Self_"));
    }

    /// A field must be snake_case or the *generated* crate warns, so a spec
    /// written in camelCase is converted rather than passed through.
    #[test]
    fn field_ident_is_snake_case() {
        assert_eq!(to_field_ident("first_name").as_deref(), Some("first_name"));
        assert_eq!(to_field_ident("firstName").as_deref(), Some("first_name"));
        assert_eq!(to_field_ident("userID").as_deref(), Some("user_id"));
        assert_eq!(
            to_field_ident("HTTPProxyURL").as_deref(),
            Some("http_proxy_url")
        );
        assert_eq!(to_field_ident("utf8Mode").as_deref(), Some("utf8_mode"));
        assert_eq!(to_field_ident("a__b").as_deref(), Some("a_b"));
        // Keywords stay as they are; `escape_keyword` makes them `r#type`.
        assert_eq!(to_field_ident("type").as_deref(), Some("type"));
    }

    #[test]
    fn snake_case_splits_words_not_letters() {
        assert_eq!(to_snake_case("PascalCase"), "pascal_case");
        assert_eq!(to_snake_case("HTTPProxyURL"), "http_proxy_url");
        assert_eq!(to_snake_case("userID"), "user_id");
        assert_eq!(to_snake_case("first-name"), "first_name");
        assert_eq!(to_snake_case("10min"), "10min");
        assert_eq!(to_snake_case("utf8"), "utf8");
        assert_eq!(to_snake_case("!!!"), "");
    }

    #[test]
    fn field_ident_rewrites_names_that_are_not_identifiers() {
        assert_eq!(to_field_ident("first-name").as_deref(), Some("first_name"));
        assert_eq!(to_field_ident("10min").as_deref(), Some("_10min"));
        assert_eq!(to_field_ident("a.b").as_deref(), Some("a_b"));
        assert_eq!(to_field_ident("with space").as_deref(), Some("with_space"));
        assert_eq!(to_field_ident("a..b").as_deref(), Some("a_b"));
        assert_eq!(to_field_ident(".lead").as_deref(), Some("lead"));
        assert_eq!(to_field_ident("trail.").as_deref(), Some("trail"));
        assert_eq!(to_field_ident("self").as_deref(), Some("self_"));
    }

    #[test]
    fn every_sanitized_name_is_a_valid_ident() {
        for raw in [
            "10min",
            "a.b",
            "with space",
            "self",
            "Self",
            "crate",
            "1",
            "é",
            "-x-",
            "9lives",
        ] {
            for (what, ident) in [
                ("type", to_type_ident(raw)),
                ("variant", to_variant_ident(raw)),
                ("field", to_field_ident(raw)),
            ] {
                let ident = ident.unwrap_or_else(|| panic!("no {what} ident for {raw:?}"));
                assert!(is_valid_ident(&ident), "{what} ident for {raw:?}: {ident}");
            }
        }
    }

    /// A name with nothing to build on has no answer, so the sanitizers say so
    /// rather than invent one.
    #[test]
    fn a_name_with_nothing_to_build_on_has_no_ident() {
        for raw in ["", "!!!", "_", "...", "-"] {
            assert_eq!(to_type_ident(raw), None, "type ident for {raw:?}");
            assert_eq!(to_variant_ident(raw), None, "variant ident for {raw:?}");
            assert_eq!(to_field_ident(raw), None, "field ident for {raw:?}");
        }
    }

    #[test]
    fn valid_ident_rejects_non_identifiers() {
        assert!(!is_valid_ident(""));
        assert!(!is_valid_ident("_"));
        assert!(!is_valid_ident("10min"));
        assert!(!is_valid_ident("a.b"));
        assert!(!is_valid_ident("with space"));
        assert!(is_valid_ident("_10min"));
        assert!(is_valid_ident("a_b"));
    }

    #[test]
    #[should_panic(expected = "not a valid Rust identifier")]
    fn assert_ident_panics_on_an_invalid_name() {
        assert_ident("10min");
    }
}
