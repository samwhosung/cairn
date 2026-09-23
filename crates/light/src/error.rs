use std::fmt;

/// Why a lighting table could not be loaded.
#[derive(Debug)]
pub enum Error {
    /// The chain has no readable copy of the table.
    Read {
        table: &'static str,
        source: mpq::ChainError,
    },
    /// The table does not parse under its schema.
    Parse {
        table: &'static str,
        source: dbc::Error,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { table, source } => write!(f, "reading {table}: {source}"),
            Self::Parse { table, source } => write!(f, "parsing {table}: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
        }
    }
}
