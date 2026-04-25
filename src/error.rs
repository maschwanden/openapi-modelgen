pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    YamlDeserialization(serde_yaml::Error),
    Formatting(std::fmt::Error),
    Io(std::io::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::YamlDeserialization(e) => write!(f, "Deserialization error: {e}"),
            Error::Formatting(e) => write!(f, "Formatting error: {e}"),
            Error::Io(e) => write!(f, "I/O error: {e}"),
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
