use std::io::Cursor;

use super::{BodyLayout, checked_layout, u32_zero_padded};
use crate::{
    DbcParser, Error, FieldType, RecordSet, Schema, SchemaField, StringRef, Value, export_to_csv,
};

fn build_wdbc(
    record_count: u32,
    field_count: u32,
    record_size: u32,
    records: &[u8],
    strings: &[u8],
) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"WDBC");
    b.extend_from_slice(&record_count.to_le_bytes());
    b.extend_from_slice(&field_count.to_le_bytes());
    b.extend_from_slice(&record_size.to_le_bytes());
    b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
    b.extend_from_slice(records);
    b.extend_from_slice(strings);
    b
}

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn id_name_schema() -> Schema {
    let mut s = Schema::new("Test");
    s.add_field(SchemaField::new("id", FieldType::UInt32));
    s.add_field(SchemaField::new("name", FieldType::String));
    s
}

fn decode(bytes: &[u8], schema: Schema) -> RecordSet {
    DbcParser::parse(&mut Cursor::new(bytes))
        .expect("header")
        .with_schema(schema)
        .expect("schema")
        .parse_records()
        .expect("records")
}

fn string_at(rs: &RecordSet, record: usize, column: usize) -> String {
    let Some(Value::StringRef(r)) = rs.records()[record].get_value(column).copied() else {
        panic!("expected a string ref");
    };
    rs.get_string(r).expect("in bounds").into_owned()
}

#[test]
fn parses_minimal_wdbc() {
    let bytes = build_wdbc(2, 2, 8, &words(&[1, 0, 2, 6]), b"Alice\0Bob\0");
    let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).expect("parses");
    assert_eq!(parser.header().record_count, 2);
    assert_eq!(parser.header().field_count, 2);

    let rs = decode(&bytes, id_name_schema());
    assert_eq!(rs.records().len(), 2);
    assert_eq!(rs.records()[0].get_value(0), Some(&Value::UInt32(1)));
    assert_eq!(string_at(&rs, 0, 1), "Alice");
    assert_eq!(rs.records()[1].get_value(0), Some(&Value::UInt32(2)));
    assert_eq!(string_at(&rs, 1, 1), "Bob");
}

#[test]
fn padded_read_zero_extends_past_the_end() {
    let b = [0xAA, 0xBB];
    assert_eq!(u32_zero_padded(&b, 0), 0x0000_BBAA);
    assert_eq!(u32_zero_padded(&b, 1), 0x0000_00BB);
    assert_eq!(u32_zero_padded(&b, 2), 0);
    assert_eq!(u32_zero_padded(&b, 5), 0);
}

#[test]
fn short_records_read_into_the_next() {
    let bytes = build_wdbc(2, 2, 4, &words(&[1, 2]), &[]);
    let mut schema = Schema::new("Short");
    schema.add_field(SchemaField::new_array("v", FieldType::UInt32, 2));
    let rs = decode(&bytes, schema);
    assert_eq!(rs.records()[0].get_value(1), Some(&Value::UInt32(2)));
    assert_eq!(rs.records()[1].get_value(1), Some(&Value::UInt32(0)));
}

#[test]
fn layout_overflow_is_none() {
    assert_eq!(checked_layout(usize::MAX, 2, 0), None);
    assert_eq!(checked_layout(4, 4, usize::MAX), None);
    assert_eq!(checked_layout(usize::MAX, 1, 1), None);
    assert_eq!(
        checked_layout(10, 4, 6),
        Some(BodyLayout {
            records_len: 40,
            end: 46
        })
    );
    let bytes = build_wdbc(0, 0, 0, &[], &[]);
    assert!(DbcParser::parse(&mut Cursor::new(bytes.as_slice())).is_ok());
}

#[test]
fn truncated_body_is_an_error() {
    let mut bytes = build_wdbc(2, 2, 8, &[0u8; 8], &[]);
    bytes[16..20].copy_from_slice(&10u32.to_le_bytes());
    assert!(matches!(
        DbcParser::parse(&mut Cursor::new(bytes.as_slice())),
        Err(Error::Truncated("records + string block"))
    ));
}

#[test]
fn short_or_foreign_files_are_not_wdbc() {
    for bytes in [&[][..], &[0u8; 19], b"XXXX\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"] {
        assert!(matches!(
            DbcParser::parse(&mut Cursor::new(bytes)),
            Err(Error::NotWdbc)
        ));
    }
}

#[test]
fn schema_field_count_must_match_header() {
    let bytes = build_wdbc(1, 2, 8, &[0u8; 8], &[]);
    let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).expect("parses");
    let mut wide = Schema::new("Wide");
    for name in ["a", "b", "c"] {
        wide.add_field(SchemaField::new(name, FieldType::UInt32));
    }
    assert!(matches!(
        parser.with_schema(wide),
        Err(Error::SchemaFieldMismatch { schema: 3, file: 2 })
    ));
}

#[test]
fn parse_records_needs_a_schema() {
    let bytes = build_wdbc(0, 0, 0, &[], &[]);
    let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).expect("parses");
    assert!(matches!(
        parser.parse_records(),
        Err(Error::Truncated("no schema"))
    ));
}

#[test]
fn array_field_expands_to_consecutive_columns() {
    let bytes = build_wdbc(1, 4, 16, &words(&[7, 10, 20, 30]), &[]);
    let mut schema = Schema::new("Arr");
    schema.add_field(SchemaField::new("id", FieldType::UInt32));
    schema.add_field(SchemaField::new_array("coords", FieldType::UInt32, 3));
    let rs = decode(&bytes, schema);
    assert_eq!(
        rs.field_names,
        ["id", "coords[0]", "coords[1]", "coords[2]"]
    );
    let r = &rs.records()[0];
    assert_eq!(r.get_value(0), Some(&Value::UInt32(7)));
    assert_eq!(r.get_value(1), Some(&Value::UInt32(10)));
    assert_eq!(r.get_value(3), Some(&Value::UInt32(30)));
    assert_eq!(r.get_value(4), None);
}

#[test]
fn int32_and_float32_fields_decode() {
    let bytes = build_wdbc(1, 2, 8, &words(&[(-5i32) as u32, 1.5f32.to_bits()]), &[]);
    let mut schema = Schema::new("Nums");
    schema.add_field(SchemaField::new("signed", FieldType::Int32));
    schema.add_field(SchemaField::new("ratio", FieldType::Float32));
    let rs = decode(&bytes, schema);
    let r = &rs.records()[0];
    assert_eq!(r.get_value(0), Some(&Value::Int32(-5)));
    assert_eq!(r.get_value(1), Some(&Value::Float32(1.5)));
}

#[test]
fn get_string_bounds_and_unterminated_tail() {
    let strings = b"hi\0tail";
    let bytes = build_wdbc(1, 1, 4, &words(&[0]), strings);
    let mut schema = Schema::new("S");
    schema.add_field(SchemaField::new("name", FieldType::String));
    let rs = decode(&bytes, schema);
    let len = strings.len() as u32;
    assert_eq!(rs.get_string(StringRef(0)).expect("in bounds"), "hi");
    assert_eq!(rs.get_string(StringRef(3)).expect("in bounds"), "tail");
    assert_eq!(rs.get_string(StringRef(len)).expect("in bounds"), "");
    assert!(matches!(
        rs.get_string(StringRef(len + 1)),
        Err(Error::BadStringRef(_))
    ));
}

#[test]
fn csv_resolves_strings_and_quotes_per_rfc4180() {
    let strings = b"plain\0a,b\0he said \"hi\"\0line1\nline2\0";
    let bytes = build_wdbc(4, 2, 8, &words(&[1, 0, 2, 6, 3, 10, 4, 23]), strings);
    let rs = decode(&bytes, id_name_schema());
    let mut out = Vec::new();
    export_to_csv(&rs, &mut out).expect("writes");
    let expected = "id,name\n\
         1,plain\n\
         2,\"a,b\"\n\
         3,\"he said \"\"hi\"\"\"\n\
         4,\"line1\nline2\"\n";
    assert_eq!(String::from_utf8(out).expect("utf-8"), expected);
}

#[test]
fn zero_size_records_are_refused_not_iterated() {
    let bytes = build_wdbc(u32::MAX, 2, 0, &[], &[]);
    let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .expect("zero-byte records fit any size")
        .with_schema(id_name_schema())
        .expect("schema matches");
    assert!(matches!(parser.parse_records(), Err(Error::Truncated(_))));
    let bytes = build_wdbc(0, 2, 0, &[], &[]);
    assert!(decode(&bytes, id_name_schema()).records().is_empty());
}
