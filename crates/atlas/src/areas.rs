use std::collections::HashMap;
use std::io::Cursor;

use dbc::{DbcParser, FieldType, Record, Schema, SchemaField, Value};
use mpq::Chain;

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";
const COLUMNS: usize = 25;
const ID: usize = 0;
const MAP: usize = 1;
const PARENT: usize = 2;
const NAME: usize = 11;
const MAX_PARENTS: usize = 7;

/// One `AreaTable` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Area {
    /// The `Map.dbc` id the area lies on.
    pub map: u32,
    /// The area it lies in; `None` for a zone.
    pub parent: Option<u32>,
    pub name: String,
}

/// The install's areas by their `AreaTable` id.
pub struct Areas(HashMap<u32, Area>);

impl Areas {
    pub fn load(chain: &Chain) -> Result<Self, String> {
        let bytes = chain
            .read(AREA_TABLE)
            .map_err(|e| format!("reading {AREA_TABLE}: {e}"))?;
        let mut schema = Schema::new("AreaTable");
        for i in 0..COLUMNS {
            let ty = if i == NAME {
                FieldType::String
            } else {
                FieldType::UInt32
            };
            schema.add_field(SchemaField::new("", ty));
        }
        let rows = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .and_then(|p| p.with_schema(schema))
            .and_then(|p| p.parse_records())
            .map_err(|e| format!("parsing {AREA_TABLE}: {e}"))?;
        let u32_at = |r: &Record, i: usize| match r.get_value(i) {
            Some(Value::UInt32(v)) => Some(*v),
            _ => None,
        };
        let areas = rows.records().iter().filter_map(|r| {
            let Some(Value::StringRef(name)) = r.get_value(NAME) else {
                return None;
            };
            let area = Area {
                map: u32_at(r, MAP)?,
                parent: Some(u32_at(r, PARENT)?).filter(|&p| p != 0),
                name: rows.get_string(*name).ok()?.into_owned(),
            };
            Some((u32_at(r, ID)?, area))
        });
        Ok(Self::from_rows(areas))
    }

    pub fn from_rows(rows: impl IntoIterator<Item = (u32, Area)>) -> Self {
        Self(rows.into_iter().collect())
    }

    pub fn get(&self, id: u32) -> Option<&Area> {
        self.0.get(&id)
    }

    /// The zone named `name` in any case, on `map` when one is given; the lower id when two
    /// share the name.
    pub fn zone_named(&self, name: &str, map: Option<u32>) -> Option<u32> {
        self.0
            .iter()
            .filter(|(_, a)| a.parent.is_none() && a.name.eq_ignore_ascii_case(name))
            .filter(|(_, a)| map.is_none_or(|m| a.map == m))
            .map(|(&id, _)| id)
            .min()
    }

    /// The zone `area` lies in, `area` itself for a zone. `None` when an unknown area is on the
    /// way up, or no zone is within seven parents.
    pub fn top_zone(&self, area: u32) -> Option<u32> {
        let mut id = area;
        for _ in 0..=MAX_PARENTS {
            match self.0.get(&id)?.parent {
                None => return Some(id),
                Some(parent) => id = parent,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(parent: u32, name: &str) -> Area {
        Area {
            map: 0,
            parent: Some(parent).filter(|&p| p != 0),
            name: name.into(),
        }
    }

    #[test]
    fn a_name_finds_a_zone_and_never_an_area_inside_one() {
        let areas = Areas::from_rows([
            (40, area(12, "Elwynn Forest")),
            (12, area(0, "Elwynn Forest")),
            (9, area(12, "Northshire Valley")),
            (7, area(0, "Twin")),
            (5, area(0, "Twin")),
        ]);
        assert_eq!(areas.zone_named("elwynn forest", None), Some(12));
        assert_eq!(areas.zone_named("Northshire Valley", None), None);
        assert_eq!(areas.zone_named("Twin", None), Some(5));
        assert_eq!(areas.zone_named("Westfall", None), None);
    }

    #[test]
    fn a_name_on_two_maps_finds_the_zone_on_the_map_given() {
        let instance = Area {
            map: 36,
            ..area(0, "Westfall")
        };
        let areas = Areas::from_rows([(206, instance), (40, area(0, "Westfall"))]);
        assert_eq!(areas.zone_named("Westfall", None), Some(40));
        assert_eq!(areas.zone_named("Westfall", Some(0)), Some(40));
        assert_eq!(areas.zone_named("Westfall", Some(36)), Some(206));
        assert_eq!(areas.zone_named("Westfall", Some(1)), None);
    }

    #[test]
    fn an_area_climbs_to_its_zone_but_not_forever() {
        let mut rows: Vec<(u32, Area)> = (1..=9).map(|id| (id, area(id - 1, "a"))).collect();
        rows.push((20, area(21, "loop")));
        rows.push((21, area(20, "loop")));
        let areas = Areas::from_rows(rows);
        assert_eq!(areas.top_zone(1), Some(1));
        assert_eq!(areas.top_zone(8), Some(1));
        assert_eq!(areas.top_zone(9), None, "eight parents up");
        assert_eq!(areas.top_zone(20), None, "a loop");
        assert_eq!(areas.top_zone(99), None, "unknown");
    }
}
