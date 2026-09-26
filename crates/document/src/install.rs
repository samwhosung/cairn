use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;

use dbc::{DbcParser, FieldType, Record, Schema, SchemaField, Value};
use mpq::Chain;

use crate::zone::Borrow;

/// A model's or building's box in its own space, in yards, before it is scaled and turned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl ModelBox {
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

/// What a zone needs of the player's install, which is read in place and never copied.
pub trait Install {
    fn model_box(&mut self, model: &str) -> Result<ModelBox, String>;
    fn has_texture(&mut self, path: &str) -> Result<bool, String>;
    /// The top-level zone called `name`, matched blind to case.
    fn zone_named(&mut self, name: &str) -> Result<Borrow, String>;
}

/// The name the archives hold a model under: `.mdx` and `.mdl` are read as `.m2`.
pub fn m2_file(model: &str) -> String {
    let l = model.to_ascii_lowercase();
    match l.strip_suffix(".mdx").or_else(|| l.strip_suffix(".mdl")) {
        Some(s) => format!("{}.m2", &model[..s.len()]),
        None => model.to_owned(),
    }
}

pub fn is_building(model: &str) -> bool {
    model.to_ascii_lowercase().ends_with(".wmo")
}

/// An install's archives, opened the first time something is asked of them.
pub struct Archives {
    data: Option<PathBuf>,
    chain: Option<Chain>,
    boxes: BTreeMap<String, ModelBox>,
    zones: Option<Vec<(u32, String)>>,
}

impl Archives {
    pub fn from_env() -> Archives {
        Archives::at(std::env::var_os("WOW_DATA").map(PathBuf::from))
    }

    pub fn at(data: Option<PathBuf>) -> Archives {
        Archives {
            data,
            chain: None,
            boxes: BTreeMap::new(),
            zones: None,
        }
    }

    pub fn chain(&mut self) -> Result<&Chain, String> {
        if self.chain.is_none() {
            let data = self
                .data
                .as_ref()
                .ok_or("set WOW_DATA to the Data directory of a WoW 1.12.1 install")?;
            let chain = Chain::open(data)
                .map_err(|e| format!("opening the install at {}: {e}", data.display()))?;
            self.chain = Some(chain);
        }
        self.chain.as_ref().ok_or_else(|| "no install".to_owned())
    }

    fn read(&mut self, path: &str) -> Result<Vec<u8>, String> {
        self.chain()?
            .read(path)
            .map_err(|e| format!("{path}: not in the install ({e})"))
    }
}

fn key(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

impl Install for Archives {
    fn model_box(&mut self, model: &str) -> Result<ModelBox, String> {
        if let Some(b) = self.boxes.get(&key(model)) {
            return Ok(*b);
        }
        let b = if is_building(model) {
            let bytes = self.read(model)?;
            match wmo::parse_wmo(&bytes) {
                Ok(wmo::ParsedWmo::Root(r)) => ModelBox {
                    min: r.bounds[0],
                    max: r.bounds[1],
                },
                Ok(wmo::ParsedWmo::Group(_)) => {
                    return Err(format!("{model}: a building's group; name its root"));
                }
                Err(e) => return Err(format!("{model}: {e}")),
            }
        } else {
            let bytes = self.read(&m2_file(model))?;
            let m = m2::parse_m2(&mut Cursor::new(bytes.as_slice()))
                .map_err(|e| format!("{model}: not an M2 model ({e})"))?;
            let bounds = &m.model().bounds;
            ModelBox {
                min: bounds.bounding_box_min,
                max: bounds.bounding_box_max,
            }
        };
        if b.min.iter().chain(&b.max).any(|v| !v.is_finite()) {
            return Err(format!("{model}: its box is not finite"));
        }
        self.boxes.insert(key(model), b);
        Ok(b)
    }

    fn has_texture(&mut self, path: &str) -> Result<bool, String> {
        Ok(self.chain()?.contains(path))
    }

    fn zone_named(&mut self, name: &str) -> Result<Borrow, String> {
        if self.zones.is_none() {
            let bytes = self.read(AREA_TABLE)?;
            self.zones = Some(top_zones(&bytes)?);
        }
        let zones = self.zones.as_deref().unwrap_or_default();
        zones
            .iter()
            .filter(|(_, n)| n.eq_ignore_ascii_case(name.trim()))
            .min_by_key(|(id, _)| *id)
            .map(|(area, name)| Borrow {
                area: *area,
                name: name.clone(),
            })
            .ok_or_else(|| format!("the install has no zone named {name:?}"))
    }
}

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";
const AREA_COLUMNS: usize = 25;
const AREA_ID: usize = 0;
const AREA_PARENT: usize = 2;
const AREA_NAME: usize = 11;

fn top_zones(bytes: &[u8]) -> Result<Vec<(u32, String)>, String> {
    let mut schema = Schema::new("AreaTable");
    for i in 0..AREA_COLUMNS {
        let ty = if i == AREA_NAME {
            FieldType::String
        } else {
            FieldType::UInt32
        };
        schema.add_field(SchemaField::new("", ty));
    }
    let rows = DbcParser::parse(&mut Cursor::new(bytes))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .map_err(|e| format!("reading {AREA_TABLE}: {e}"))?;
    let number = |r: &Record, i: usize| match r.get_value(i) {
        Some(Value::UInt32(v)) => Some(*v),
        _ => None,
    };
    Ok(rows
        .records()
        .iter()
        .filter(|r| number(r, AREA_PARENT) == Some(0))
        .filter_map(|r| {
            let Some(Value::StringRef(name)) = r.get_value(AREA_NAME) else {
                return None;
            };
            Some((
                number(r, AREA_ID)?,
                rows.get_string(*name).ok()?.into_owned(),
            ))
        })
        .collect())
}
