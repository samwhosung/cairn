use std::fmt;

/// Why an ADT could not be read.
#[derive(Debug)]
pub enum Error {
    /// The named structure is shorter than its fixed size.
    Truncated(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(what) => write!(f, "truncated ADT: {what}"),
        }
    }
}

impl std::error::Error for Error {}
