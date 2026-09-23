use std::fmt;

/// Why bytes could not be read as a WMO file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Neither a root (`MOHD`) nor a group (`MOGP`) chunk was found.
    NotWmo,
    /// The named chunk's payload is shorter than its format requires.
    Truncated(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWmo => f.write_str("not a WMO (no MOHD root or MOGP group chunk)"),
            Self::Truncated(chunk) => write!(f, "truncated WMO chunk: {chunk}"),
        }
    }
}

impl std::error::Error for Error {}
