use std::io::Cursor;

use bevy::prelude::Resource;
use dbc::{DbcParser, FieldType, RecordSet, Schema, SchemaField, Value};
use mpq::Chain;

const MAP_DBC: &str = "DBFilesClient\\Map.dbc";
const MAP_FIELDS: usize = 42;

/// The map the world is on: its `Map.dbc` id, and the directory its files are under.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct CurrentMap {
    pub id: u32,
    pub directory: String,
}

impl CurrentMap {
    /// The map `name` names in `Map.dbc`: by id, or by directory in any case.
    pub fn find(chain: &Chain, name: &str) -> Result<Self, String> {
        let bytes = chain.read(MAP_DBC).map_err(|e| e.to_string())?;
        let maps = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .and_then(|p| p.with_schema(schema()))
            .and_then(|p| p.parse_records())
            .map_err(|e| format!("{MAP_DBC}: {e}"))?;
        let wanted_id = name.trim().parse::<u32>().ok();
        rows(&maps)
            .find(|map| {
                wanted_id.map_or_else(
                    || map.directory.eq_ignore_ascii_case(name.trim()),
                    |id| map.id == id,
                )
            })
            .ok_or_else(|| format!("no map {name} in {MAP_DBC}"))
    }
}

fn rows(maps: &RecordSet) -> impl Iterator<Item = CurrentMap> + '_ {
    maps.records().iter().filter_map(|r| {
        let (Some(Value::UInt32(id)), Some(Value::StringRef(dir))) =
            (r.get_value(0), r.get_value(1))
        else {
            return None;
        };
        let directory = maps.get_string(*dir).ok()?.into_owned();
        Some(CurrentMap { id: *id, directory })
    })
}

fn schema() -> Schema {
    let mut schema = Schema::new("Map");
    schema.add_field(SchemaField::new("id", FieldType::UInt32));
    schema.add_field(SchemaField::new("directory", FieldType::String));
    schema.add_field(SchemaField::new_array(
        "unread",
        FieldType::UInt32,
        MAP_FIELDS - 2,
    ));
    schema
}
