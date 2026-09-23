use std::fmt;

/// Why terrain could not be built or loaded.
#[derive(Debug)]
pub enum Error {
    Adt(adt::Error),
    /// The ADT parsed, but no chunk has a full height grid.
    NoTerrain,
    Read {
        path: String,
        source: mpq::ChainError,
    },
    Wdt {
        path: String,
        source: wdt::Error,
    },
    /// No tile near the position has terrain.
    NoTileNear {
        map: String,
        x: f32,
        y: f32,
    },
    /// No tile within the radius both exists and builds.
    NoTilesAround {
        map: String,
        x: f32,
        y: f32,
        radius: u32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adt(e) => write!(f, "parsing ADT: {e}"),
            Self::NoTerrain => f.write_str("ADT produced no terrain chunks"),
            Self::Read { path, source } => write!(f, "reading {path}: {source}"),
            Self::Wdt { path, source } => write!(f, "parsing WDT {path}: {source}"),
            Self::NoTileNear { map, x, y } => {
                write!(f, "no existing ADT tile near world ({x}, {y}) on map {map}")
            }
            Self::NoTilesAround { map, x, y, radius } => {
                write!(
                    f,
                    "no loadable tiles within {radius} of world ({x}, {y}) on {map}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Adt(e) => Some(e),
            Self::Read { source, .. } => Some(source),
            Self::Wdt { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<adt::Error> for Error {
    fn from(e: adt::Error) -> Self {
        Self::Adt(e)
    }
}
