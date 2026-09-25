//! The sound tables, read off the install's patch chain.

mod areas;
mod footsteps;
mod kits;
mod providers;
mod voices;
mod water;
mod weapons;

use std::fmt;
use std::io::Cursor;

use dbc::{DbcParser, FieldType, Record, RecordSet, Schema, SchemaField, Value};
use mpq::Chain;

pub use areas::{AreaAudio, AreaSounds, ZoneIntro, ZoneMusic};
pub use footsteps::Footsteps;
pub use kits::{Kit, KitCatalog, kit_flags};
pub use providers::{SoundProvider, SoundProviders};
pub use voices::{CreatureVoices, Exertion, Injury, Voice};
pub use water::WaterSounds;
pub use weapons::{Impact, SwingWeight, WeaponSounds, impact_slot};

/// Why a sound table could not be loaded.
#[derive(Debug)]
pub enum Error {
    Read {
        table: &'static str,
        source: mpq::ChainError,
    },
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

pub(crate) fn read_table(
    chain: &Chain,
    table: &'static str,
    columns: &[(&str, FieldType)],
) -> Result<RecordSet, Error> {
    let bytes = chain
        .read(table)
        .map_err(|source| Error::Read { table, source })?;
    let mut schema = Schema::new(table);
    for &(name, ty) in columns {
        schema.add_field(SchemaField::new(name, ty));
    }
    DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|source| Error::Parse { table, source })
}

pub(crate) fn u32_columns<const N: usize>() -> [(&'static str, FieldType); N] {
    [("", FieldType::UInt32); N]
}

pub(crate) fn u32_at(r: &Record, i: usize) -> Option<u32> {
    match r.get_value(i)? {
        Value::UInt32(v) => Some(*v),
        _ => None,
    }
}

pub(crate) fn f32_at(r: &Record, i: usize) -> Option<f32> {
    match r.get_value(i)? {
        Value::Float32(v) => Some(*v),
        _ => None,
    }
}

pub(crate) fn str_at(rs: &RecordSet, r: &Record, i: usize) -> String {
    match r.get_value(i) {
        Some(Value::StringRef(at)) => rs
            .get_string(*at)
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        _ => String::new(),
    }
}
