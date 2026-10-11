use std::fmt;

/// Error raised by sigdb.
///
/// `Format` and `Integrity` mirror `sigdb.FormatError` and `sigdb.IntegrityError` of the Python
/// package, with the same messages. `Io` covers what Python reports as `OSError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Format(String),
    Integrity(String),
    Io(std::io::ErrorKind, String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    pub(crate) fn format(message: impl Into<String>) -> Self {
        Error::Format(message.into())
    }

    pub(crate) fn integrity(message: impl Into<String>) -> Self {
        Error::Integrity(message.into())
    }

    /// Class name of the matching Python exception: `FormatError`, `IntegrityError` or `OSError`.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Error::Format(_) => "FormatError",
            Error::Integrity(_) => "IntegrityError",
            Error::Io(..) => "OSError",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Error::Format(m) | Error::Integrity(m) | Error::Io(_, m) => m,
        }
    }

    pub fn is_format(&self) -> bool {
        matches!(self, Error::Format(_))
    }

    pub fn is_integrity(&self) -> bool {
        matches!(self, Error::Integrity(_))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.kind(), e.to_string())
    }
}
