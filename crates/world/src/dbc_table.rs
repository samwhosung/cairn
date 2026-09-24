//! The client's tables, read off the install column by column.

use std::io::Cursor;

use dbc::{DbcParser, FieldType, Record, RecordSet, Schema, SchemaField, Value};
use mpq::Chain;

/// `table` read as `columns` columns, each a `u32` but the strings at `strings`.
pub(crate) fn read_table(
    chain: &Chain,
    table: &str,
    columns: usize,
    strings: &[usize],
) -> Result<RecordSet, String> {
    let bytes = chain
        .read(table)
        .map_err(|e| format!("reading {table}: {e}"))?;
    let mut schema = Schema::new(table);
    for i in 0..columns {
        let ty = if strings.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new("", ty));
    }
    DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("parsing {table}: {e}"))
}

pub(crate) fn u32_at(r: &Record, i: usize) -> Option<u32> {
    match r.get_value(i)? {
        Value::UInt32(v) => Some(*v),
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
