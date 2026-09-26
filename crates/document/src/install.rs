use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use dbc::{DbcParser, FieldType, Record, Schema, SchemaField, Value};
pub use fits::Rules;
use mpq::Chain;

use crate::relief::{self, Source};
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
    /// How the install places a model on its ground, as the catalog counted it; `None` for a model
    /// it never places there, or with no catalog to ask.
    fn rules(&mut self, model: &str) -> Result<Option<Rules>, String>;
    /// The relief of the top-level zone called `name`.
    fn relief(&mut self, name: &str) -> Result<Arc<Source>, String>;
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

/// An install's archives, opened the first time something is asked of them, and the catalog
/// `cairn catalog` wrote of it, for the models' rules.
pub struct Archives {
    data: Option<PathBuf>,
    catalog: Option<PathBuf>,
    chain: Option<Chain>,
    boxes: BTreeMap<String, ModelBox>,
    areas: Option<Areas>,
    book: Option<fits::rules::Book>,
    reliefs: BTreeMap<String, Arc<Source>>,
}

impl Archives {
    /// The install at `$WOW_DATA` and the catalog at `$CAIRN_CATALOG`.
    pub fn from_env() -> Archives {
        Archives::at(std::env::var_os("WOW_DATA").map(PathBuf::from))
            .with_catalog(std::env::var_os("CAIRN_CATALOG").map(PathBuf::from))
    }

    pub fn at(data: Option<PathBuf>) -> Archives {
        Archives {
            data,
            catalog: None,
            chain: None,
            boxes: BTreeMap::new(),
            areas: None,
            book: None,
            reliefs: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_catalog(mut self, catalog: Option<PathBuf>) -> Archives {
        self.catalog = catalog;
        self.book = None;
        self
    }

    fn areas(&mut self) -> Result<&Areas, String> {
        if self.areas.is_none() {
            let bytes = self.read(AREA_TABLE)?;
            self.areas = Some(Areas::parse(&bytes)?);
        }
        self.areas.as_ref().ok_or_else(|| "no areas".to_owned())
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
        let zone = self.areas()?.zone_named(name)?;
        Ok(Borrow {
            area: zone.id,
            name: zone.name.clone(),
        })
    }

    fn rules(&mut self, model: &str) -> Result<Option<Rules>, String> {
        let Some(catalog) = &self.catalog else {
            return Ok(None);
        };
        if self.book.is_none() {
            self.book = Some(fits::rules::read(&catalog.join(fits::IN_CATALOG))?);
        }
        Ok(self.book.as_ref().and_then(|b| b.of(model)))
    }

    fn relief(&mut self, name: &str) -> Result<Arc<Source>, String> {
        let key = name.trim().to_ascii_lowercase();
        if let Some(s) = self.reliefs.get(&key) {
            return Ok(Arc::clone(s));
        }
        let zone = self.areas()?.zone_named(name)?.name.clone();
        self.chain()?;
        let (Some(chain), Some(areas)) = (&self.chain, &self.areas) else {
            return Err("no install".into());
        };
        let source = Arc::new(Source::of(&zone, &relief::read(chain, areas, &zone)?)?);
        self.reliefs.insert(key, Arc::clone(&source));
        Ok(source)
    }
}

const AREA_TABLE: &str = "DBFilesClient\\AreaTable.dbc";
const AREA_COLUMNS: usize = 25;
const AREA_ID: usize = 0;
const AREA_MAP: usize = 1;
const AREA_PARENT: usize = 2;
const AREA_NAME: usize = 11;
const MOST_PARENTS: usize = 7;

/// An `AreaTable` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Area {
    pub id: u32,
    pub map: u32,
    pub parent: u32,
    pub name: String,
}

pub struct Areas(BTreeMap<u32, Area>);

impl Areas {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Areas, String> {
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
        Ok(Areas(
            rows.records()
                .iter()
                .filter_map(|r| {
                    let Some(Value::StringRef(name)) = r.get_value(AREA_NAME) else {
                        return None;
                    };
                    let area = Area {
                        id: number(r, AREA_ID)?,
                        map: number(r, AREA_MAP)?,
                        parent: number(r, AREA_PARENT)?,
                        name: rows.get_string(*name).ok()?.into_owned(),
                    };
                    Some((area.id, area))
                })
                .collect(),
        ))
    }

    /// The top-level zone called `name`, matched blind to case; the lowest id of two.
    pub fn zone_named(&self, name: &str) -> Result<&Area, String> {
        self.0
            .values()
            .find(|a| a.parent == 0 && a.name.eq_ignore_ascii_case(name.trim()))
            .ok_or_else(|| format!("the install has no zone named {name:?}"))
    }

    /// The top-level zone `area` lies in, itself for one; `None` for an area the install lacks or
    /// one more than seven parents deep.
    pub fn top_zone(&self, area: u32) -> Option<u32> {
        let mut id = area;
        for _ in 0..=MOST_PARENTS {
            match self.0.get(&id)?.parent {
                0 => return Some(id),
                parent => id = parent,
            }
        }
        None
    }
}
