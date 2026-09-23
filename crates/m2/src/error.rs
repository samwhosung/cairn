use std::fmt;

/// Why a model, or one of its skin profiles, could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The file does not start with `MD20`.
    NotMd20,
    /// An MD20 version outside the 256..=263 this reader knows.
    UnsupportedVersion(u32),
    /// A header, array or record runs past the end of the file, or a skin profile index is out
    /// of range.
    Truncated,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotMd20 => f.write_str("not an MD20 model"),
            Self::UnsupportedVersion(v) => {
                write!(f, "unsupported M2 version {v} (expected 256..=263)")
            }
            Self::Truncated => f.write_str("truncated M2"),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) type Result<T> = std::result::Result<T, Error>;
