use std::fmt;

use crate::{DialRanges, StartOutfitItem};

/// Why a table or a texture could not be loaded, or a table read wrong.
#[derive(Debug)]
pub enum Error {
    /// The chain has no readable copy of the file.
    Read {
        path: String,
        source: mpq::ChainError,
    },
    /// A table does not parse under its schema.
    Parse {
        table: &'static str,
        source: dbc::Error,
    },
    /// A texture the BLP decoder refuses.
    Decode { path: String, source: blp::Error },
    /// A byte-packed table with no `WDBC` header.
    NotWdbc { table: &'static str },
    /// A byte-packed table whose header gives another layout than the one it is read with.
    Layout {
        table: &'static str,
        fields: u32,
        record_size: u32,
    },
    /// A byte-packed table that ends inside a record.
    RecordPastEnd { table: &'static str, record: usize },
    /// `CharBaseInfo` gives a race other classes than the shipped table does.
    Classes {
        race: u8,
        got: Vec<u8>,
        known: &'static [u8],
    },
    /// `ChrRaces` names a race other than the shipped table does.
    RaceFile {
        race: u8,
        got: Option<String>,
        known: &'static str,
    },
    /// The Human Warrior's starting outfit lacks an item the shipped table gives it.
    StartOutfit {
        display_id: u32,
        inv_type: u8,
        got: Vec<StartOutfitItem>,
    },
    /// A playable race has no body display for a sex.
    NoBodyDisplay { race: u8, sex: u8 },
    /// A playable race has no customization tokens.
    NoCustomizationTokens { race: u8 },
    /// A playable race has a customization dial with no values for a sex.
    EmptyDial {
        race: u8,
        sex: u8,
        ranges: DialRanges,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "reading {path}: {source}"),
            Self::Parse { table, source } => write!(f, "parsing {table}: {source}"),
            Self::Decode { path, source } => write!(f, "decoding {path}: {source}"),
            Self::NotWdbc { table } => write!(f, "{table}: not a WDBC file"),
            Self::Layout {
                table,
                fields,
                record_size,
            } => write!(
                f,
                "{table}: {fields} fields in {record_size}-byte records is not the layout it is read with"
            ),
            Self::RecordPastEnd { table, record } => {
                write!(f, "{table}: record {record} runs past the file")
            }
            Self::Classes { race, got, known } => write!(
                f,
                "CharBaseInfo gives race {race} classes {got:?}, the shipped table {known:?}"
            ),
            Self::RaceFile { race, got, known } => write!(
                f,
                "ChrRaces names race {race} {got:?}, the shipped table {known:?}"
            ),
            Self::StartOutfit {
                display_id,
                inv_type,
                got,
            } => write!(
                f,
                "CharStartOutfit dresses the Human Warrior in {got:?}, without display \
                 {display_id} at inventory type {inv_type}"
            ),
            Self::NoBodyDisplay { race, sex } => {
                write!(f, "ChrRaces: race {race} sex {sex} has no body display")
            }
            Self::NoCustomizationTokens { race } => {
                write!(f, "ChrRaces: race {race} has no customization tokens")
            }
            Self::EmptyDial { race, sex, ranges } => write!(
                f,
                "race {race} sex {sex} has a customization dial with no values: {ranges:?}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
            _ => None,
        }
    }
}
