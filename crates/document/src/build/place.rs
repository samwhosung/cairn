//! Placed models and buildings as the ADT keeps them: `MDDF` and `MODF` records with their name
//! tables, and the chunks that list each. The client creates a tile's placements through its
//! chunks' `MCRF` lists, so a chunk lists every placement whose box, turned and scaled, meets its
//! square, edges included. A placement whose box crosses a tile edge is in every tile it meets.

use crate::frame::{CENTRE, CHUNK};
use crate::install::ModelBox;

/// A placement's turn in world axes: `Rz(rotation[1] + 180°) · Ry(rotation[0]) · Rx(rotation[2])`.
fn turn(rotation: [f32; 3], v: [f64; 3]) -> [f64; 3] {
    let (about_y, about_z, about_x) = (
        f64::from(rotation[0]).to_radians(),
        f64::from(rotation[1]).to_radians() + std::f64::consts::PI,
        f64::from(rotation[2]).to_radians(),
    );
    let (sn, cs) = (libm::sin(about_x), libm::cos(about_x));
    let v = [v[0], cs * v[1] - sn * v[2], sn * v[1] + cs * v[2]];
    let (sn, cs) = (libm::sin(about_y), libm::cos(about_y));
    let v = [cs * v[0] + sn * v[2], v[1], -sn * v[0] + cs * v[2]];
    let (sn, cs) = (libm::sin(about_z), libm::cos(about_z));
    [cs * v[0] - sn * v[1], sn * v[0] + cs * v[1], v[2]]
}

/// World `(x north, y west, z)` of a placement-frame position.
fn world(position: [f32; 3]) -> [f64; 3] {
    [
        CENTRE - f64::from(position[2]),
        CENTRE - f64::from(position[0]),
        f64::from(position[1]),
    ]
}

/// The world ground a placement covers: its box's x (north) and y (west) ranges.
pub struct Cover {
    pub x: [f64; 2],
    pub y: [f64; 2],
}

impl Cover {
    /// Does the chunk whose north-west corner is at world `(x, y)` meet it?
    pub fn meets_chunk(&self, x: f64, y: f64) -> bool {
        self.x[0] <= x && self.x[1] >= x - CHUNK && self.y[0] <= y && self.y[1] >= y - CHUNK
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Doodad {
    pub mmdx_name: String,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: u16,
    pub model_box: ModelBox,
}

impl Doodad {
    pub fn cover(&self) -> Cover {
        let p = world(self.position);
        let s = f64::from(self.scale) / 1024.0;
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for k in 0..8 {
            let v = turn(self.rotation, corner(&self.model_box, k));
            for i in 0..2 {
                lo[i] = lo[i].min(p[i] + s * v[i]);
                hi[i] = hi[i].max(p[i] + s * v[i]);
            }
        }
        Cover {
            x: [lo[0], hi[0]],
            y: [lo[1], hi[1]],
        }
    }
}

fn corner(b: &ModelBox, k: usize) -> [f64; 3] {
    [
        f64::from(if k & 1 == 0 { b.min[0] } else { b.max[0] }),
        f64::from(if k & 2 == 0 { b.min[1] } else { b.max[1] }),
        f64::from(if k & 4 == 0 { b.min[2] } else { b.max[2] }),
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wmo {
    pub model: String,
    pub unique_id: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    /// Its box turned and moved to its place, in the placement frame, lower then upper.
    pub bounds: [[f32; 3]; 2],
    pub doodad_set: u16,
}

impl Wmo {
    pub fn bounds_of(position: [f32; 3], rotation: [f32; 3], b: &ModelBox) -> [[f32; 3]; 2] {
        let pos = world(position);
        let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
        for k in 0..8 {
            let v = turn(rotation, corner(b, k));
            let w = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
            let p = [CENTRE - w[1], w[2], CENTRE - w[0]];
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        [lo.map(|v| v as f32), hi.map(|v| v as f32)]
    }

    pub fn cover(&self) -> Cover {
        let [lo, hi] = self.bounds;
        Cover {
            x: [CENTRE - f64::from(hi[2]), CENTRE - f64::from(lo[2])],
            y: [CENTRE - f64::from(hi[0]), CENTRE - f64::from(lo[0])],
        }
    }
}

/// A tile's placements of one kind as its three records keep them.
pub struct Tables {
    /// Each model's name once, in order of first use, `MMDX` or `MWMO`.
    pub names: Vec<u8>,
    /// Where each name starts, `MMID` or `MWID`.
    pub starts: Vec<u8>,
    /// `MDDF` or `MODF`.
    pub records: Vec<u8>,
}

fn named<'a>(models: impl Iterator<Item = &'a str>) -> (Tables, Vec<u32>) {
    let mut t = Tables {
        names: Vec::new(),
        starts: Vec::new(),
        records: Vec::new(),
    };
    let (mut seen, mut index): (Vec<&str>, Vec<u32>) = (Vec::new(), Vec::new());
    for m in models {
        let i = seen.iter().position(|n| *n == m).unwrap_or_else(|| {
            seen.push(m);
            t.starts
                .extend_from_slice(&(t.names.len() as u32).to_le_bytes());
            t.names.extend_from_slice(m.as_bytes());
            t.names.push(0);
            seen.len() - 1
        });
        index.push(i as u32);
    }
    (t, index)
}

pub fn doodad_tables(list: &[Doodad]) -> Tables {
    let (mut t, index) = named(list.iter().map(|d| d.mmdx_name.as_str()));
    for (d, i) in list.iter().zip(index) {
        t.records.extend_from_slice(&i.to_le_bytes());
        t.records.extend_from_slice(&d.unique_id.to_le_bytes());
        for v in d.position.iter().chain(&d.rotation) {
            t.records.extend_from_slice(&v.to_le_bytes());
        }
        t.records.extend_from_slice(&d.scale.to_le_bytes());
        t.records.extend_from_slice(&0u16.to_le_bytes());
    }
    t
}

pub fn wmo_tables(list: &[Wmo]) -> Tables {
    let (mut t, index) = named(list.iter().map(|w| w.model.as_str()));
    for (w, i) in list.iter().zip(index) {
        t.records.extend_from_slice(&i.to_le_bytes());
        t.records.extend_from_slice(&w.unique_id.to_le_bytes());
        for v in w
            .position
            .iter()
            .chain(&w.rotation)
            .chain(&w.bounds[0])
            .chain(&w.bounds[1])
        {
            t.records.extend_from_slice(&v.to_le_bytes());
        }
        for v in [0, w.doodad_set, 0, 0u16] {
            t.records.extend_from_slice(&v.to_le_bytes());
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doodad(name: &str, rotation: [f32; 3]) -> Doodad {
        Doodad {
            mmdx_name: name.into(),
            unique_id: 1,
            position: [100.0, 0.0, 200.0],
            rotation,
            scale: 1024,
            model_box: ModelBox {
                min: [-5.0, -1.0, 0.0],
                max: [5.0, 1.0, 3.0],
            },
        }
    }

    #[test]
    fn a_box_turns_with_its_heading() {
        let c = doodad("A.mdx", [0.0; 3]).cover();
        assert!((c.x[1] - c.x[0] - 10.0).abs() < 1e-6 && (c.y[1] - c.y[0] - 2.0).abs() < 1e-6);
        let c = doodad("A.mdx", [0.0, 90.0, 0.0]).cover();
        assert!((c.x[1] - c.x[0] - 2.0).abs() < 1e-6 && (c.y[1] - c.y[0] - 10.0).abs() < 1e-6);
    }

    #[test]
    fn tables_name_each_model_once() {
        let d = |m: &str| doodad(m, [0.0; 3]);
        let t = doodad_tables(&[d("A.mdx"), d("B.mdx"), d("A.mdx")]);
        assert_eq!(t.names, b"A.mdx\0B.mdx\0");
        assert_eq!(t.starts, [0, 0, 0, 0, 6, 0, 0, 0]);
        assert_eq!(t.records.len(), 108);
        assert_eq!(&t.records[72..76], &[0, 0, 0, 0]);
    }
}
