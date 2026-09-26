//! Values typed in are held at the precision the text shows them (hundredths of a yard and a
//! degree, 1/1024 of a scale), so a zone read back from its files is the zone that wrote them.

use std::collections::BTreeMap;
use std::fmt;

use crate::frame::{CELL, Frame};
use crate::shape::Shape;

pub const TEXELS_ACROSS: usize = 64;
pub const TEXELS_IN_CHUNK: usize = TEXELS_ACROSS * TEXELS_ACROSS;
/// The most textures a chunk can blend: the format's budget.
pub const MAX_LAYERS: usize = 4;
const COUNT_BITS: u32 = 24;
pub const MADE_LIMIT: u32 = (1 << COUNT_BITS) - 1;
pub const AUTHOR_LIMIT: usize = (1 << (32 - COUNT_BITS)) - 1;

/// Every vertex's height: the outer lattice at cell corners, `(cols + 1) × (rows + 1)`, and the
/// inner at cell centres, `cols × rows`, row-major, rows stepping south.
#[derive(Clone, Debug, PartialEq)]
pub struct Heights {
    pub cols: usize,
    pub rows: usize,
    pub outer: Vec<f32>,
    pub inner: Vec<f32>,
}

impl Heights {
    pub fn flat(cols: usize, rows: usize, z: f32) -> Heights {
        Heights {
            cols,
            rows,
            outer: vec![z; (cols + 1) * (rows + 1)],
            inner: vec![z; cols * rows],
        }
    }

    pub fn outer(&self, i: usize, j: usize) -> f32 {
        self.outer[j * (self.cols + 1) + i]
    }

    pub fn inner(&self, i: usize, j: usize) -> f32 {
        self.inner[j * self.cols + i]
    }

    /// Vertices of both lattices, the outer first: the index a vertex has in [`Heights::get`].
    pub fn len(&self) -> usize {
        self.outer.len() + self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, k: usize) -> f32 {
        match k.checked_sub(self.outer.len()) {
            None => self.outer[k],
            Some(k) => self.inner[k],
        }
    }

    pub fn set(&mut self, k: usize, v: f32) {
        match k.checked_sub(self.outer.len()) {
            None => self.outer[k] = v,
            Some(k) => self.inner[k] = v,
        }
    }

    pub fn point(&self, k: usize) -> [f64; 2] {
        match k.checked_sub(self.outer.len()) {
            None => {
                let (i, j) = (k % (self.cols + 1), k / (self.cols + 1));
                [i as f64 * CELL, j as f64 * CELL]
            }
            Some(k) => {
                let (i, j) = (k % self.cols, k / self.cols);
                [(i as f64 + 0.5) * CELL, (j as f64 + 0.5) * CELL]
            }
        }
    }

    /// The ground's height at a point as the client draws it, each cell a fan of four triangles
    /// from its centre; a point off the zone takes its nearest edge.
    pub fn ground(&self, p: [f64; 2]) -> f64 {
        let (u, v) = (p[0] / CELL, p[1] / CELL);
        let ci = (u.floor().max(0.0) as usize).min(self.cols - 1);
        let cj = (v.floor().max(0.0) as usize).min(self.rows - 1);
        let (fu, fv) = (
            (u - ci as f64).clamp(0.0, 1.0),
            (v - cj as f64).clamp(0.0, 1.0),
        );
        let tl = [0.0, 0.0, f64::from(self.outer(ci, cj))];
        let tr = [1.0, 0.0, f64::from(self.outer(ci + 1, cj))];
        let bl = [0.0, 1.0, f64::from(self.outer(ci, cj + 1))];
        let br = [1.0, 1.0, f64::from(self.outer(ci + 1, cj + 1))];
        let c = [0.5, 0.5, f64::from(self.inner(ci, cj))];
        let (dx, dy) = (fu - 0.5, fv - 0.5);
        let (a, b) = if dy.abs() >= dx.abs() {
            if dy < 0.0 { (tl, tr) } else { (bl, br) }
        } else if dx < 0.0 {
            (tl, bl)
        } else {
            (tr, br)
        };
        let det = (a[1] - b[1]) * (c[0] - b[0]) + (b[0] - a[0]) * (c[1] - b[1]);
        let l1 = ((a[1] - b[1]) * (fu - b[0]) + (b[0] - a[0]) * (fv - b[1])) / det;
        let l2 = ((b[1] - c[1]) * (fu - b[0]) + (c[0] - b[0]) * (fv - b[1])) / det;
        let l3 = 1.0 - l1 - l2;
        l1 * c[2] + l2 * a[2] + l3 * b[2]
    }

    /// How far the ground rises over a yard east and a yard south at a point, from the corners of
    /// the cell holding it; at a cell's middle, the cell's own slope.
    pub fn rise(&self, p: [f64; 2]) -> [f64; 2] {
        let (u, v) = (p[0] / CELL, p[1] / CELL);
        let ci = (u.floor().max(0.0) as usize).min(self.cols - 1);
        let cj = (v.floor().max(0.0) as usize).min(self.rows - 1);
        let (fu, fv) = (
            (u - ci as f64).clamp(0.0, 1.0),
            (v - cj as f64).clamp(0.0, 1.0),
        );
        let h = |i: usize, j: usize| f64::from(self.outer(ci + i, cj + j));
        let east = ((h(1, 0) - h(0, 0)) * (1.0 - fv) + (h(1, 1) - h(0, 1)) * fv) / CELL;
        let south = ((h(0, 1) - h(0, 0)) * (1.0 - fu) + (h(1, 1) - h(1, 0)) * fu) / CELL;
        [east, south]
    }

    /// The slope at a point in degrees, of [`Heights::rise`].
    pub fn slope(&self, p: [f64; 2]) -> f64 {
        let [east, south] = self.rise(p);
        libm::atan(east.hypot(south)).to_degrees()
    }
}

/// One texture's weights over a chunk's texels, row-major, rows stepping south.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintLayer {
    pub palette_place: u16,
    pub w: Vec<u8>,
}

/// A chunk's paint: its textures in the order they arrived, the first the chunk's base layer,
/// their weights summing to 255 on every texel.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkPaint {
    pub layers: Vec<PaintLayer>,
}

/// A ground texture the zone paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    pub path: String,
    /// Its `GroundEffectTexture` id: the grass and pebbles the client grows on it and the sound of
    /// footsteps on it. 0 is none.
    pub effect: u32,
}

/// Where a thing stands in height, in hundredths of a yard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Z {
    /// This far above the ground under it, following the ground.
    Ground(i64),
    /// At this world height.
    At(i64),
}

/// A placed model or building.
#[derive(Clone, Debug, PartialEq)]
pub struct Thing {
    pub model: String,
    /// Hundredths of a yard.
    pub x: i64,
    pub y: i64,
    pub z: Z,
    /// The compass bearing the model's front faces, in hundredths of a degree.
    pub facing: i64,
    /// 1024 is 1.0, as the ADT keeps it.
    pub scale: u16,
    /// A building's doodad set, shown beside set 0; `None` for a model.
    pub set: Option<u16>,
    /// Tilted as the ground under it is, rather than upright.
    pub lean: bool,
    /// The seed of the scatter that placed it; `None` once placed or moved by hand.
    pub scattered: Option<u64>,
}

impl Thing {
    pub fn is_building(&self) -> bool {
        self.set.is_some()
    }

    pub fn at(&self) -> [f64; 2] {
        [self.x as f64 / 100.0, self.y as f64 / 100.0]
    }

    pub fn facing_deg(&self) -> f64 {
        self.facing as f64 / 100.0
    }

    pub fn scale_f(&self) -> f64 {
        f64::from(self.scale) / 1024.0
    }

    pub fn world_z(&self, heights: &Heights) -> f64 {
        match self.z {
            Z::Ground(dz) => heights.ground(self.at()) + dz as f64 / 100.0,
            Z::At(z) => z as f64 / 100.0,
        }
    }
}

/// A body of water: a level, in hundredths of a yard, over an outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Water {
    pub level: i64,
    pub shape: Shape,
}

/// A thing's or a body of water's id: its author, and that author's count when it was made.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    pub author: String,
    pub n: u32,
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.author, self.n)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name: String,
    /// Its map's directory, `World\Maps\<map>`.
    pub map: String,
    pub start: Option<Start>,
    pub borrow: Option<Borrow>,
}

/// Where a player starts, in hundredths of a yard and of a degree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Start {
    pub x: i64,
    pub y: i64,
    pub facing: i64,
}

/// The install's zone a zone takes its light, sky, water colour and music from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Borrow {
    /// Its `AreaTable` id.
    pub area: u32,
    pub name: String,
}

/// Someone who has changed the zone, and how many ids they have handed out. Neither is ever
/// taken back, so no id is handed out twice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Author {
    pub name: String,
    pub made: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub frame: Frame,
    pub settings: Settings,
    pub authors: Vec<Author>,
    pub heights: Heights,
    /// Per chunk, row-major over the zone.
    pub paint: Vec<ChunkPaint>,
    pub palette: Vec<Texture>,
    pub things: BTreeMap<Id, Thing>,
    pub water: BTreeMap<Id, Water>,
}

impl Zone {
    /// A new zone: flat at `z`, painted with one texture.
    pub fn new(name: &str, frame: Frame, z: f32, texture: Texture) -> Zone {
        let (cols, rows) = frame.cells();
        let (east, south) = frame.chunks();
        let whole = ChunkPaint {
            layers: vec![PaintLayer {
                palette_place: 0,
                w: vec![255; TEXELS_IN_CHUNK],
            }],
        };
        Zone {
            frame,
            settings: Settings {
                name: name.to_owned(),
                map: map_dir(name),
                start: None,
                borrow: None,
            },
            authors: Vec::new(),
            heights: Heights::flat(cols, rows, z),
            paint: vec![whole; east * south],
            palette: vec![texture],
            things: BTreeMap::new(),
            water: BTreeMap::new(),
        }
    }

    pub fn chunks_east(&self) -> usize {
        self.frame.chunks().0
    }

    /// Matched blind to case and to which slash it uses.
    pub fn palette_place(&self, path: &str) -> Option<u16> {
        let norm = |s: &str| s.replace('/', "\\").to_ascii_lowercase();
        let want = norm(path);
        self.palette
            .iter()
            .position(|t| norm(&t.path) == want)
            .map(|i| i as u16)
    }

    /// The number a thing's unique id carries for its author: its place among the zone's authors.
    pub fn author_number(&self, name: &str) -> Option<u32> {
        self.authors
            .iter()
            .position(|a| a.name == name)
            .map(|i| i as u32 + 1)
    }

    /// The ADT's unique id of a thing: its author's number, then its count.
    pub fn unique_id(&self, id: &Id) -> Option<u32> {
        Some(self.author_number(&id.author)? << COUNT_BITS | id.n)
    }

    pub fn id_of_unique(&self, unique: u32) -> Option<Id> {
        let author = self
            .authors
            .get((unique >> COUNT_BITS).checked_sub(1)? as usize)?;
        Some(Id {
            author: author.name.clone(),
            n: unique & MADE_LIMIT,
        })
    }
}

/// A map directory for a zone's name: its letters, digits and `_`, from a letter.
pub fn map_dir(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    match s.chars().next() {
        None => "Zone".to_owned(),
        Some(c) if !c.is_ascii_alphabetic() => format!("Z{s}"),
        Some(_) => s,
    }
}

/// An author's name: a letter, then letters, digits, `_` or `-`, at most 32 in all.
pub fn check_author(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let fine = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        && name.len() <= 32;
    if fine {
        Ok(())
    } else {
        Err(format!(
            "{name:?} is not an author's name: a letter, then letters, digits, _ or -, at most 32"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ground_is_the_fan_of_four() {
        let mut h = Heights::flat(2, 2, 0.0);
        h.inner[0] = 4.0;
        assert!((h.ground([CELL * 0.5, CELL * 0.5]) - 4.0).abs() < 1e-9);
        assert!(h.ground([0.0, 0.0]).abs() < 1e-9);
        assert!((h.ground([CELL * 0.5, CELL * 0.25]) - 2.0).abs() < 1e-9);
        let k = h.outer.len() + 3;
        let p = h.point(k);
        assert!((p[0] - 1.5 * CELL).abs() < 1e-9 && (p[1] - 1.5 * CELL).abs() < 1e-9);
    }

    #[test]
    fn the_slope_is_the_cells_from_its_corners() {
        let mut h = Heights::flat(3, 3, 0.0);
        let rise = libm::tan(30f64.to_radians());
        for j in 0..=3 {
            for i in 0..=3 {
                h.outer[j * 4 + i] = (i as f64 * CELL * rise) as f32;
            }
        }
        h.inner[4] = 50.0;
        for p in [
            [0.2 * CELL, 1.5 * CELL],
            [1.5 * CELL, 1.5 * CELL],
            [2.9 * CELL, 0.1],
        ] {
            assert!((h.slope(p) - 30.0).abs() < 1e-4, "{p:?}: {}", h.slope(p));
            assert!(h.rise(p)[1].abs() < 1e-9);
        }
    }

    #[test]
    fn unique_ids_carry_the_author_and_the_count() {
        let mut z = Zone::new(
            "A",
            Frame {
                origin: (32, 48),
                size: (1, 1),
            },
            0.0,
            Texture {
                path: "t.blp".into(),
                effect: 0,
            },
        );
        for name in ["sam", "ai"] {
            z.authors.push(Author {
                name: name.into(),
                made: 0,
            });
        }
        let id = Id {
            author: "ai".into(),
            n: 12,
        };
        let unique = z.unique_id(&id).expect("a known author");
        assert_eq!(unique, 0x0200_000C);
        assert_eq!(z.id_of_unique(unique), Some(id));
        assert_eq!(map_dir("9 Wells!"), "Z9Wells");
        assert!(check_author("ai-2").is_ok() && check_author("2ai").is_err());
    }
}
