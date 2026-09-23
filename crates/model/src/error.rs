use std::fmt;

/// Why a model could not be loaded.
#[derive(Debug)]
pub enum Error {
    /// The chain could not read the file.
    Chain(mpq::ChainError),
    M2(m2::Error),
    /// The model parsed but its first skin profile did not.
    Skin(m2::Error),
    Wmo(wmo::Error),
    /// The file is a WMO group where a root was expected.
    NotWmoRoot,
    Blp(blp::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chain(e) => e.fmt(f),
            Self::M2(e) => write!(f, "parsing M2: {e}"),
            Self::Skin(e) => write!(f, "embedded skin: {e}"),
            Self::Wmo(e) => write!(f, "parsing WMO: {e}"),
            Self::NotWmoRoot => f.write_str("not a WMO root file"),
            Self::Blp(e) => write!(f, "decoding BLP: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Chain(e) => Some(e),
            Self::M2(e) | Self::Skin(e) => Some(e),
            Self::Wmo(e) => Some(e),
            Self::NotWmoRoot => None,
            Self::Blp(e) => Some(e),
        }
    }
}
