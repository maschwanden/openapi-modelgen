//! A field's `default` value: whether the generator can emit it, and as what.

use crate::{Diagnostic, diagnostic::Severity};

/// Take a field's `default`, if the generator can emit it.
///
/// `enum_values` holds the spec values of the field's inline enum, and is
/// `None` for every other field. `context` is the field's spec location,
/// where a rejected default is reported.
pub(super) fn extract_default(
    raw: &Option<serde_json::Value>,
    rust_type: &str,
    enum_values: Option<&[String]>,
    context: &str,
) -> (Option<serde_json::Value>, Vec<Diagnostic>) {
    let Some(value) = raw.as_ref() else {
        return (None, Vec::new());
    };
    match classify_default(value, rust_type, enum_values) {
        DefaultVerdict::Usable => (Some(value.clone()), Vec::new()),
        DefaultVerdict::Unrenderable => (
            None,
            vec![Diagnostic::new(
                Severity::Degraded,
                context,
                "default value",
                format!(
                    "default value {value} ignored (type `{rust_type}` does not support code-generated defaults)"
                ),
            )],
        ),
        DefaultVerdict::Mismatch(reason) => (
            None,
            vec![Diagnostic::new(
                Severity::Fatal,
                context,
                "default value",
                format!("default value {value} {reason}"),
            )],
        ),
    }
}

/// What the generator can do with a `default` value.
enum DefaultVerdict {
    /// The value fits the field's type, and [`crate::write`] has a literal for it.
    Usable,
    /// The value fits the field's type, but the generator cannot write it as a
    /// Rust literal. An array default, or one on a field that already degraded
    /// to `serde_json::Value`. The gap is the generator's, so the default is
    /// dropped and the field keeps its `Option`.
    Unrenderable,
    /// The value is not one the field's type can hold, so the spec contradicts
    /// itself. No output is honest: emitting the default writes a value the
    /// type cannot represent, and dropping it makes the field required. Carries
    /// the reason, which completes "default value X ...".
    Mismatch(String),
}

/// Decide what to do with a field's `default`.
///
/// `enum_values` holds the spec values of the field's inline enum, and is
/// `None` for every other field.
///
/// A [`DefaultVerdict::Usable`] verdict promises that
/// `write::format_default_literal` can render the value, so the two must stay
/// in step: the writer panics rather than emit a field whose `#[serde(default)]`
/// went missing.
fn classify_default(
    value: &serde_json::Value,
    rust_type: &str,
    enum_values: Option<&[String]>,
) -> DefaultVerdict {
    use DefaultVerdict::{Mismatch, Unrenderable, Usable};

    // The enum's Rust name is not final here (`write` prefixes it with the
    // struct's), so the message names the values instead of a type the user
    // will not find in the output.
    if let Some(values) = enum_values {
        return match value {
            serde_json::Value::String(s) if values.iter().any(|v| v == s) => Usable,
            _ => Mismatch(format!(
                "is not one of the enum's values ({})",
                values.join(", ")
            )),
        };
    }

    let fits = |ok: bool, detail: &str| {
        if ok {
            Usable
        } else {
            Mismatch(format!("is not valid for type `{rust_type}`: {detail}"))
        }
    };

    match rust_type {
        "String" => fits(value.is_string(), "not a string"),
        "bool" => fits(value.is_boolean(), "not a boolean"),
        "f64" => fits(value.is_number(), "not a number"),
        // An i32 literal outside the type's range does not compile, so the
        // width is part of the check, not just integer-ness.
        "i32" | "i64" => match value.as_i64() {
            Some(n) if rust_type == "i32" && i32::try_from(n).is_err() => {
                fits(false, "outside the range of `i32`")
            }
            Some(_) => Usable,
            None => fits(false, "not an integer"),
        },
        // Parsed here so the `.expect()` in the generated default function is
        // unreachable: a malformed literal would otherwise panic at runtime,
        // the first time a payload omits the field.
        "DateTime<Utc>" => fits(
            value
                .as_str()
                .is_some_and(|s| s.parse::<chrono::DateTime<chrono::Utc>>().is_ok()),
            "not an RFC 3339 date-time",
        ),
        "NaiveDate" => fits(
            value
                .as_str()
                .is_some_and(|s| s.parse::<chrono::NaiveDate>().is_ok()),
            "not an ISO 8601 date",
        ),
        "Uuid" => fits(
            value
                .as_str()
                .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()),
            "not a UUID",
        ),
        // The field's own type was already degraded, so every JSON value fits it.
        "serde_json::Value" => Unrenderable,
        t if t.starts_with("Vec<") => match value {
            serde_json::Value::Array(_) => Unrenderable,
            _ => fits(false, "not an array"),
        },
        // A struct type, reached only through a `$ref`, and a `$ref` sibling
        // `default` is ignored before it arrives here.
        _ => Unrenderable,
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use crate::{diagnostic::Severity, parse::testutil::*};

    /// An inline-enum `default` that names no variant is dropped with a
    /// diagnostic (rather than emitting a nonexistent, uncompilable variant).
    #[test]
    fn diagnostic_bad_enum_default_is_fatal() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      properties:
        status:
          type: string
          enum: [active, inactive]
          default: unknown
"
        ))?;
        let d = find_diag(&diags, "default value");
        assert_eq!(d.severity, Severity::Fatal);
        assert_eq!(d.path, "Foo.status");
        assert_eq!(
            d.reason,
            "default value \"unknown\" is not one of the enum's values (active, inactive)"
        );

        Ok(())
    }

    /// A `default` the declared type cannot hold is fatal: emitting it writes a
    /// value the type cannot represent, and dropping it makes the field
    /// required, so neither output matches the spec.
    #[test]
    fn diagnostic_mismatched_default_is_fatal() -> Result<()> {
        let cases = [
            (
                "{type: integer, default: 1.5}",
                "1.5",
                "`i64`: not an integer",
            ),
            (
                "{type: integer, format: int32, default: 3000000000}",
                "3000000000",
                "`i32`: outside the range of `i32`",
            ),
            (
                r#"{type: integer, default: "abc"}"#,
                r#""abc""#,
                "`i64`: not an integer",
            ),
            (
                r#"{type: string, format: date-time, default: "not-a-date"}"#,
                r#""not-a-date""#,
                "`DateTime<Utc>`: not an RFC 3339 date-time",
            ),
            (
                r#"{type: string, format: date, default: "31.12.2024"}"#,
                r#""31.12.2024""#,
                "`NaiveDate`: not an ISO 8601 date",
            ),
            (
                r#"{type: string, format: uuid, default: "zzz"}"#,
                r#""zzz""#,
                "`Uuid`: not a UUID",
            ),
            (
                r#"{type: boolean, default: "yes"}"#,
                r#""yes""#,
                "`bool`: not a boolean",
            ),
            (
                r#"{type: array, items: {type: string}, default: "nope"}"#,
                r#""nope""#,
                "`Vec<String>`: not an array",
            ),
        ];

        for (schema, value, detail) in cases {
            let diags = diagnostics_for(&format!(
                r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      properties:
        prop: {schema}
"
            ))?;
            let d = find_diag(&diags, "default value");
            assert_eq!(d.severity, Severity::Fatal, "for {schema}");
            assert_eq!(d.path, "Foo.prop", "for {schema}");
            assert_eq!(
                d.reason,
                format!("default value {value} is not valid for type {detail}"),
            );
        }

        Ok(())
    }

    /// A `default` the type *can* hold but the generator cannot write stays
    /// degraded: the gap is the generator's, and ignoring it keeps the field
    /// `Option<T>`, which still matches the spec.
    #[test]
    fn diagnostic_unrenderable_default_is_degraded() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      properties:
        tags:
          type: array
          items:
            type: string
          default: [a, b]
"
        ))?;
        let d = find_diag(&diags, "default value");
        assert_eq!(d.severity, Severity::Degraded);
        assert_eq!(d.path, "Foo.tags");
        assert_eq!(
            d.reason,
            "default value [\"a\",\"b\"] ignored \
             (type `Vec<String>` does not support code-generated defaults)"
        );

        Ok(())
    }

    /// A valid inline-enum `default` produces no diagnostic.
    #[test]
    fn diagnostic_good_enum_default_is_clean() -> Result<()> {
        let diags = diagnostics_for(&format!(
            r"{MINIMAL_HEADER}
paths: {{}}
components:
  schemas:
    Foo:
      type: object
      properties:
        status:
          type: string
          enum: [active, inactive]
          default: active
"
        ))?;
        assert!(
            !has_construct(&diags, "default value"),
            "valid enum default should be clean, got {diags:?}"
        );

        Ok(())
    }
}
