use std::fmt;
use std::io;

/// Why a WDT could not be read.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The file ends partway through a chunk header.
    TruncatedHeader,
    /// A chunk claims more bytes than the file has left.
    ChunkPastEnd,
    /// The file has no `MAIN` tile table.
    MissingMain,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::TruncatedHeader => f.write_str("truncated WDT chunk header"),
            Self::ChunkPastEnd => f.write_str("WDT chunk runs past the end of the stream"),
            Self::MissingMain => f.write_str("WDT missing MAIN chunk"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
