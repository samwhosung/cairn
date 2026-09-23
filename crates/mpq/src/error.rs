use std::fmt;
use std::io;
use std::path::PathBuf;

/// Why an archive could not be opened, or a file in it read.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The file has no MPQ header.
    NotMpq,
    /// The archive has no readable file by this name.
    NotFound(String),
    /// A file or codec that 1.12.1 archives do not use.
    Unsupported(String),
    /// A sector could not be decoded.
    Decompress(String),
    /// A header or table value the archive cannot satisfy, such as a size past its end.
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::NotMpq => f.write_str("not an MPQ archive (no header signature found)"),
            Self::NotFound(name) => write!(f, "file not in archive: {name}"),
            Self::Unsupported(what) => write!(f, "unsupported: {what}"),
            Self::Decompress(what) => write!(f, "decompress: {what}"),
            Self::Corrupt(what) => write!(f, "corrupt archive: {what}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Why a chain could not be opened, or a file in it read.
#[derive(Debug)]
pub enum ChainError {
    /// The directory could not be listed.
    List { dir: PathBuf, source: io::Error },
    /// An archive could not be opened.
    Open { path: PathBuf, source: Error },
    /// The directory holds none of the archives the client mounts.
    NoArchives(PathBuf),
    /// No archive in the chain has an entry for the file.
    NotFound(String),
    /// The file's winning entry is a delete marker.
    Deleted { name: String, archive: PathBuf },
    /// The archive that holds the file could not read it.
    Read {
        name: String,
        archive: PathBuf,
        source: Error,
    },
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::List { dir, source } => write!(f, "listing {}: {source}", dir.display()),
            Self::Open { path, source } => write!(f, "opening {}: {source}", path.display()),
            Self::NoArchives(dir) => write!(f, "no known vanilla MPQs found in {}", dir.display()),
            Self::NotFound(name) => write!(f, "file not in patch chain: {name}"),
            Self::Deleted { name, archive } => write!(
                f,
                "file deleted from patch chain: {name} (deleted by {})",
                archive.display()
            ),
            Self::Read {
                name,
                archive,
                source,
            } => write!(f, "reading {name} from {}: {source}", archive.display()),
        }
    }
}

impl std::error::Error for ChainError {}
