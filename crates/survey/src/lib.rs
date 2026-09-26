//! What the maps of a World of Warcraft 1.12.1 install paint and place, by zone: every ground
//! texture, doodad and building, with where and how often Blizzard used it.

mod font;
mod gather;
mod pages;
mod picture;
mod scan;
mod sky;
mod text;
mod words;
mod write;

use std::collections::BTreeMap;

use atlas::Doodads;
use mpq::Chain;

pub use gather::model_bounds as bounds;
pub use pages::PICTURE_SIDE;
pub use picture::{save_averaged, write_atomically};
pub use text::ZoneSound;
pub use write::{Lookups, Written, pictures_missing, write, write_pages};

/// Everything the maps paint and place, each list sorted by key. Every list inside is most first.
pub struct Survey {
    pub zones: Vec<Zone>,
    pub grounds: Vec<Ground>,
    pub models: Vec<Model>,
}

/// An `AreaTable` row with no parent, on one map; ground in no zone is area 0's.
pub struct Zone {
    pub area: u32,
    pub name: String,
    pub map: u32,
    pub map_directory: String,
    pub key: String,
    pub chunks: u32,
    pub tiles: Option<TileSpan>,
    pub wet_cells: WetCells,
    /// Its areas, itself among them, by the chunks they cover.
    pub places: Vec<(String, u32)>,
    /// Ground by the texels it shows on, 4096 a chunk.
    pub grounds: Vec<(usize, f64)>,
    pub models: Vec<Tally>,
    /// Doodads standing on its ground, each once.
    pub doodads: Doodads,
    pub buildings: u32,
}

/// The tiles holding a zone's chunks, numbered as `Map_x_y.adt` numbers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileSpan {
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WetCells {
    pub water: u32,
    pub ocean: u32,
    pub magma: u32,
    pub slime: u32,
}

impl WetCells {
    pub fn by_kind(self) -> [(&'static str, u32); 4] {
        [
            ("water", self.water),
            ("ocean", self.ocean),
            ("magma", self.magma),
            ("slime", self.slime),
        ]
    }
}

/// A ground texture the chunks paint.
pub struct Ground {
    /// As the maps spell it most often.
    pub path: String,
    pub key: String,
    /// What its name says it is.
    pub kind: &'static str,
    pub texels: f64,
    pub chunks: u32,
    pub zones: Vec<(usize, f64)>,
    /// Grounds painted in the same chunks, by the share of this one's texels there.
    pub beside: Vec<(usize, f64)>,
}

/// A doodad (an M2) or a building (a WMO) the maps place.
pub struct Model {
    pub path: String,
    pub key: String,
    pub building: bool,
    /// `building`, or what the file name says the doodad is.
    pub kind: &'static str,
    /// `[min, max]` at scale 1 in its own axes, yards: x forward, y left, z up.
    pub bounds: Option<[[f32; 3]; 2]>,
    /// One without a mesh only emits particles, light or sound, none of which shows at rest.
    pub mesh: bool,
    pub on_ground: u32,
    pub in_buildings: u32,
    pub zones: Vec<Tally>,
    pub places: Vec<Place>,
    pub scales: Option<Scales>,
    pub examples: Vec<Example>,
}

/// How often a model is placed in a zone; `index` is the model's in a zone's list, the zone's in a
/// model's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    pub index: usize,
    pub on_ground: u32,
    pub in_buildings: u32,
}

impl Tally {
    pub fn placed(self) -> u32 {
        self.on_ground + self.in_buildings
    }
}

/// An area within a zone, and how often a model is placed in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub zone: usize,
    pub area: String,
    pub placements: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scales {
    pub least: f32,
    pub p10: f32,
    pub p50: f32,
    pub p90: f32,
    pub most: f32,
}

/// One placement of a model, in world coordinates: x north, y west, z up.
#[derive(Clone, Debug, PartialEq)]
pub struct Example {
    pub map_directory: String,
    pub position: [f32; 3],
    /// Degrees about the up axis, as the map stores it.
    pub heading: f32,
    pub scale: f32,
    pub building: Option<String>,
}

fn spelling(counts: &BTreeMap<String, u32>) -> String {
    counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .map(|(s, _)| s.clone())
        .unwrap_or_default()
}

/// A file path as a key: lowercase, `/` between parts, and anything but letters, digits, `.`, `-`
/// and `_` as `_`, without its extension.
pub fn key(path: &str) -> String {
    let lower = path.to_ascii_lowercase().replace('\\', "/");
    let stem = match lower.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => stem,
        _ => lower.as_str(),
    };
    stem.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn read(chain: &Chain) -> Result<Survey, String> {
    gather::survey(chain)
}

#[cfg(test)]
mod tests;
