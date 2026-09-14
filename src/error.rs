pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    YamlDeserialization(serde_yaml::Error),
    Formatting(std::fmt::Error),
    Io(std::io::Error),
    /// Constructs the generator cannot represent at all, each carrying a
    /// [`crate::Severity::Fatal`] diagnostic.
    Unrepresentable(Vec<crate::Diagnostic>),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::YamlDeserialization(e) => write!(f, "Deserialization error: {e}"),
            Error::Formatting(e) => write!(f, "Formatting error: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Unrepresentable(diagnostics) => {
                let plural = if diagnostics.len() == 1 { "" } else { "s" };
                write!(f, "{} fatal problem{plural} in the spec", diagnostics.len())?;
                for diagnostic in diagnostics {
                    write!(f, "\n\n  {}\n    {}", diagnostic.path, diagnostic.reason)?;
                }
                write!(f, "\n\nFix the spec, then re-run. No files were written.")
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_yaml::Error> for Error {
    fn from(e: serde_yaml::Error) -> Self {
        Error::YamlDeserialization(e)
    }
}

impl From<std::fmt::Error> for Error {
    fn from(e: std::fmt::Error) -> Self {
        Error::Formatting(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
