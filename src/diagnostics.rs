//! Diagnostics for spec constructs the generator cannot fully represent.
//!
//! The generator only emits code for a subset of OpenAPI. Historically the
//! unsupported parts were dropped or degraded silently, so a spec could
//! "generate successfully" while quietly losing whole types. A [`Diagnostic`]
//! records each such loss so it can be surfaced to the user (CLI summary) and
//! to library callers (returned from [`crate::generate`]).

use std::fmt;

/// How lossy a single unsupported construct was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The construct produced no output at all (e.g. an `allOf` schema that
    /// generated no type).
    Dropped,
    /// The construct produced a lossy fallback (e.g. an inline object field
    /// degraded to `serde_json::Value`).
    Degraded,
}

impl Severity {
    /// Lowercase label used in rendered messages (e.g. `"dropped"`).
    pub fn label(self) -> &'static str {
        match self {
            Severity::Dropped => "dropped",
            Severity::Degraded => "degraded",
        }
    }
}

/// A single report of a spec construct that was not fully generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Where in the spec the construct lives, e.g. `components.schemas.Order`,
    /// `Pet.metadata`, or `GET /things#limit`.
    pub path: String,
    /// The OpenAPI construct involved, e.g. `allOf`, `inline object`,
    /// `additionalProperties`, `header parameter`.
    pub construct: String,
    /// Human-readable explanation of what happened and its consequence.
    pub reason: String,
    /// Whether the construct was dropped entirely or degraded to a fallback.
    pub severity: Severity,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} — {} ({}, {})",
            self.path,
            self.reason,
            self.construct,
            self.severity.label()
        )
    }
}

/// Record a diagnostic by pushing it onto the collector.
///
/// The collector is the single source of truth: it is returned from
/// [`crate::generate`] and logged at `warn` by the [`crate::parse`] convenience
/// wrapper. Recording does not log here — that would double-print on the CLI,
/// which renders its own summary from the returned list.
pub(crate) fn record(
    diagnostics: &mut Vec<Diagnostic>,
    severity: Severity,
    path: impl Into<String>,
    construct: impl Into<String>,
    reason: impl Into<String>,
) {
    diagnostics.push(Diagnostic {
        path: path.into(),
        construct: construct.into(),
        reason: reason.into(),
        severity,
    });
}
