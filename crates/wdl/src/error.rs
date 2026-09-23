use std::fmt;

/// Why a WDL could not be read.
#[derive(Debug)]
pub enum Error {
    /// The file does not start with an `MVER` chunk.
    NotWdl,
    /// A version other than the 18 the 1.12.1 client reads.
    Version(u32),
    /// The named structure runs past the end of the file.
    Truncated(&'static str),
    /// The `MAOF` offset table is not 64×64 entries.
    MaofSize(u32),
    /// The `MAOF` entry for tile `index` does not point at a whole `MARE` chunk.
    BadMare { index: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWdl => f.write_str("not a WDL: no MVER chunk first"),
            Self::Version(v) => write!(f, "WDL version {v}, expected 18"),
            Self::Truncated(what) => write!(f, "truncated WDL: {what}"),
            Self::MaofSize(n) => write!(f, "WDL MAOF is {n} bytes, expected 16384"),
            Self::BadMare { index } => write!(f, "WDL MAOF entry {index} is not a MARE chunk"),
        }
    }
}

impl std::error::Error for Error {}
