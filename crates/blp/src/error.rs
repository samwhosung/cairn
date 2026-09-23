use std::fmt;

use crate::header::MAX_DIM;

/// Why a texture could not be decoded.
#[derive(Debug)]
pub enum Error {
    /// Shorter than a BLP2 header, or no `BLP2` magic.
    NotBlp2,
    Truncated(&'static str),
    UnknownCompression(u8),
    /// The 256-entry palette that follows the header is cut short.
    BadColorMap,
    /// The level's offset and size run past the end of the file.
    OutOfBounds {
        level: usize,
    },
    /// Width or height over 8192.
    DimensionsTooLarge {
        width: u32,
        height: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotBlp2 => write!(f, "not a BLP2 file (bad magic)"),
            Self::Truncated(what) => write!(f, "truncated BLP: {what}"),
            Self::UnknownCompression(c) => write!(f, "unknown BLP compression {c}"),
            Self::BadColorMap => write!(f, "BLP color map shorter than 256 entries"),
            Self::OutOfBounds { level } => write!(f, "BLP mip level {level} out of bounds"),
            Self::DimensionsTooLarge { width, height } => write!(
                f,
                "BLP dimensions {width}x{height} exceed the {MAX_DIM} sanity cap"
            ),
        }
    }
}

impl std::error::Error for Error {}
