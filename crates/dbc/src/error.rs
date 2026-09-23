use std::fmt;

/// Why a DBC could not be parsed, or a string in it resolved.
#[derive(Debug)]
pub enum Error {
    /// The file is shorter than the 20-byte header or does not start with `WDBC`.
    NotWdbc,
    /// Part of the file is missing. Also returned by [`crate::DbcParser::parse_records`] when no
    /// schema was attached, and when a header claims records of zero bytes.
    Truncated(&'static str),
    /// The schema's field count, arrays expanded, differs from the header's.
    SchemaFieldMismatch { schema: usize, file: u32 },
    /// A string offset past the end of the string block.
    BadStringRef(u32),
    /// The header's sizes add up to more than `usize` holds.
    SizeOverflow,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotWdbc => f.write_str("not a WDBC file (bad magic)"),
            Self::Truncated(what) => write!(f, "truncated DBC: {what}"),
            Self::SchemaFieldMismatch { schema, file } => {
                write!(f, "schema has {schema} fields but file has {file}")
            }
            Self::BadStringRef(off) => write!(f, "string ref {off} out of bounds"),
            Self::SizeOverflow => {
                f.write_str("record_count * record_size + string_block_size overflows usize")
            }
        }
    }
}

impl std::error::Error for Error {}

pub(crate) type Result<T> = std::result::Result<T, Error>;
