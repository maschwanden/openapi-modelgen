//! Diagnostics for spec constructs the generator cannot fully represent.
//!
//! The generator only emits code for a subset of OpenAPI. To prevent
//! unsupported parts being dropped or degraded silently, [`Diagnostic`]s
//! record each such loss so they can be surfaced to the user (CLI summary) and
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
    /// The construct cannot produce any meaningful value and has no fallback
    /// available (e.g. two spec names that map to one Rust name, or a name with
    /// nothing to build one from).
    Fatal,
}

impl Severity {
    /// Lowercase label used in rendered messages.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Dropped => "dropped",
            Severity::Degraded => "degraded",
            Severity::Fatal => "fatal",
        }
    }

    /// Whether this severity aborts generation.
    pub fn is_fatal(self) -> bool {
        matches!(self, Severity::Fatal)
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

impl Diagnostic {
    /// Build a report of a construct that was not fully generated.
    ///
    /// This is the only constructor: a caller that returns a diagnostic and a
    /// caller that pushes one onto a list (through [`record`], which calls
    /// this) build the same value, so the two paths cannot drift apart.
    pub(crate) fn new(
        severity: Severity,
        path: impl Into<String>,
        construct: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Diagnostic {
            path: path.into(),
            construct: construct.into(),
            reason: reason.into(),
            severity,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} ({}, {})",
            self.path,
            self.reason,
            self.construct,
            self.severity.label()
        )
    }
}

/// Record a diagnostic by pushing it onto the caller's own list.
///
/// A function that reports more than one loss accumulates them in a list of its
/// own and returns it; one that reports at most a single loss builds that
/// [`Diagnostic`] and returns it directly instead. Recording never logs: the
/// CLI renders its own summary from the returned list, so a log line here would
/// print every diagnostic twice.
pub(crate) fn record(
    diagnostics: &mut Vec<Diagnostic>,
    severity: Severity,
    path: impl Into<String>,
    construct: impl Into<String>,
    reason: impl Into<String>,
) {
    diagnostics.push(Diagnostic::new(severity, path, construct, reason));
}
