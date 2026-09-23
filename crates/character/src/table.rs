use std::io::Cursor;

use dbc::{DbcParser, FieldType, Record, RecordSet, Schema, SchemaField, Value};
use mpq::Chain;

use crate::Error;

pub(crate) fn read(chain: &Chain, path: &str) -> Result<Vec<u8>, Error> {
    chain.read(path).map_err(|source| Error::Read {
        path: path.to_owned(),
        source,
    })
}

pub(crate) fn parse(
    table: &'static str,
    bytes: &[u8],
    columns: &[(&str, FieldType)],
) -> Result<RecordSet, Error> {
    let mut schema = Schema::new(table);
    for &(name, ty) in columns {
        schema.add_field(SchemaField::new(name, ty));
    }
    DbcParser::parse(&mut Cursor::new(bytes))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|source| Error::Parse { table, source })
}

pub(crate) fn read_table(
    chain: &Chain,
    table: &'static str,
    columns: &[(&str, FieldType)],
) -> Result<RecordSet, Error> {
    parse(table, &read(chain, table)?, columns)
}

pub(crate) fn unnamed_u32_columns<const N: usize>() -> [(&'static str, FieldType); N] {
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

pub(crate) fn nonempty_str_at(rs: &RecordSet, r: &Record, i: usize) -> Option<String> {
    match r.get_value(i)? {
        Value::StringRef(at) => {
            let s = rs.get_string(*at).ok()?;
            (!s.is_empty()).then(|| s.into_owned())
        }
        _ => None,
    }
}

/// Tables name models `.mdx`; the archives hold them as `.m2`.
pub(crate) fn model_path(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower
        .strip_suffix(".mdx")
        .or_else(|| lower.strip_suffix(".mdl"))
    {
        Some(stem) => format!("{stem}.m2"),
        None => lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_paths_name_the_m2() {
        assert_eq!(model_path("Shield_Round_A_01.mdx"), "shield_round_a_01.m2");
        assert_eq!(model_path("A.MDL"), "a.m2");
        assert_eq!(model_path("A.m2"), "a.m2");
        assert_eq!(model_path("A"), "a");
    }
}
