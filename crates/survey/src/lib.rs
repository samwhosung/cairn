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

use mpq::Chain;

pub use picture::{save_averaged, write_atomically};
pub use text::{SkyLines, ZoneSound};
pub use write::{Written, pictures_missing, write, write_pages};

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
    /// `[x0, x1, y0, y1]` of the tiles holding its chunks.
    pub tiles: Option<[u32; 4]>,
    /// The middle of the chunk nearest the middle of them all, where its sky is read.
    pub heart: Option<[f32; 3]>,
    /// Wet cells by [`WATERS`].
    pub water: [u32; 4],
    /// Its areas, itself among them, by the chunks they cover.
    pub places: Vec<(String, u32)>,
    /// Ground by the texels it shows on, 4096 a chunk.
    pub grounds: Vec<(usize, f64)>,
    /// Models placed in it: on the ground, and inside buildings.
    pub models: Vec<(usize, u32, u32)>,
    /// Doodads standing on its ground, each once, by [`atlas::Kind::ALL`].
    pub doodads: [u32; 5],
    pub buildings: u32,
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
    /// Zone, on the ground, inside buildings.
    pub zones: Vec<(usize, u32, u32)>,
    /// Zone, area within it, placements.
    pub places: Vec<(usize, String, u32)>,
    /// The least, the 10th, 50th and 90th percentiles, and the most.
    pub scales: Option<[f32; 5]>,
    pub examples: Vec<Example>,
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

pub const WATERS: [&str; 4] = scan::WATERS;

/// The most common spelling, the least of those in byte order on a tie.
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

/// The box the model at `path` fills, as [`Model::bounds`]: a building's groups, or a doodad's
/// vertices at rest.
pub fn bounds(chain: &Chain, path: &str) -> Option<[[f32; 3]; 2]> {
    gather::model_bounds(chain, path)
}

pub fn read(chain: &Chain) -> Result<Survey, String> {
    gather::survey(chain)
}

#[cfg(test)]
mod tests;
