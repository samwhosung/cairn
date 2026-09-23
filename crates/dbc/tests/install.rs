use std::io::Cursor;
use std::path::PathBuf;

use dbc::{DbcParser, FieldType, Record, RecordSet, Schema, SchemaField, StringRef, Value};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn decode(chain: &Chain, name: &str, schema: Schema) -> RecordSet {
    let bytes = chain.read(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn columns(field_count: usize, text: &[usize]) -> Schema {
    let mut schema = Schema::new("columns");
    for i in 0..field_count {
        let ty = if text.contains(&i) {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new(format!("c{i}"), ty));
    }
    schema
}

fn find(rs: &RecordSet, id: u32) -> &Record {
    rs.records()
        .iter()
        .find(|r| r.get_value(0) == Some(&Value::UInt32(id)))
        .unwrap_or_else(|| panic!("no record {id}"))
}

fn text(rs: &RecordSet, r: &Record, column: usize) -> String {
    let Some(Value::StringRef(s)) = r.get_value(column).copied() else {
        panic!("column {column} is not a string");
    };
    rs.get_string(s).expect("in bounds").into_owned()
}

#[test]
fn every_table_decodes_every_record() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let mut tables = 0;
    let mut packed = Vec::new();
    for entry in chain.list() {
        if !entry.name.to_ascii_lowercase().ends_with(".dbc") {
            continue;
        }
        let bytes = chain.read(&entry.name).expect("read");
        let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).expect("header");
        let header = *parser.header();
        if header.record_size != header.field_count * 4 {
            packed.push(entry.name.clone());
        }
        let mut schema = Schema::new("all");
        schema.add_field(SchemaField::new_array(
            "c",
            FieldType::UInt32,
            header.field_count as usize,
        ));
        let rs = parser
            .with_schema(schema)
            .and_then(|p| p.parse_records())
            .unwrap_or_else(|e| panic!("{}: {e}", entry.name));
        assert_eq!(rs.records().len(), header.record_count as usize);
        assert_eq!(rs.get_string(StringRef(0)).expect("block starts empty"), "");
        tables += 1;
    }
    assert!(tables > 150, "{tables} tables");
    packed.sort();
    assert_eq!(
        packed,
        [
            "DBFilesClient\\CharBaseInfo.dbc",
            "DBFilesClient\\CharStartOutfit.dbc"
        ],
        "tables whose fields are not all four bytes"
    );
}

#[test]
fn map_names_the_continents() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let rs = decode(&chain, "DBFilesClient\\Map.dbc", columns(42, &[1, 4]));
    assert_eq!(rs.records().len(), 44);
    assert_eq!(text(&rs, find(&rs, 0), 1), "Azeroth");
    assert_eq!(text(&rs, find(&rs, 0), 4), "Eastern Kingdoms");
    assert_eq!(text(&rs, find(&rs, 1), 1), "Kalimdor");
}

#[test]
fn spell_names_fireball() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let rs = decode(
        &chain,
        "DBFilesClient\\Spell.dbc",
        columns(173, &[120, 129, 138]),
    );
    assert_eq!(rs.records().len(), 22_357);
    let fireball = find(&rs, 133);
    assert_eq!(text(&rs, fireball, 120), "Fireball");
    assert_eq!(text(&rs, fireball, 129), "Rank 1");
    assert!(text(&rs, fireball, 138).starts_with("Hurls a fiery ball"));
}
