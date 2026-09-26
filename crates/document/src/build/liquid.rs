//! A chunk's water as the ADT keeps it: an 804-byte block after the `MCLQ` header, announced by
//! a bit of the chunk's flags. The block is `{min, max, 81 × {4 bytes, height}, 64 cell flags,
//! 84 bytes of flow}`, written as Blizzard's tool wrote river and lake water: the first byte of a
//! vertex is its depth, nine to a yard; a wet cell is `4`, with `0x40` where all four of its
//! corners are a yard deep; the flow is two empty records.

use crate::frame::{CELL, CHUNK};
use crate::zone::Zone;

pub const BLOCK: usize = 0x324;
/// The MCNK flag of a chunk with river or lake water.
pub const RIVER_FLAG: u32 = 0x04;
const CELL_RIVER: u8 = 0x04;
const CELL_DRY: u8 = 0x0F;
const DEEP_BYTE: u8 = 9;
const DEPTH_PER_YD: f32 = 9.0;

pub fn river(surface: &[f32; 81], ground: &[f32; 81], wet: &[bool; 64]) -> Vec<u8> {
    let depth: Vec<u8> = (0..81)
        .map(|i| {
            ((surface[i] - ground[i]) * DEPTH_PER_YD)
                .floor()
                .clamp(0.0, 255.0) as u8
        })
        .collect();
    let lo = surface.iter().copied().fold(f32::MAX, f32::min);
    let hi = surface.iter().copied().fold(f32::MIN, f32::max);
    let mut b = Vec::with_capacity(BLOCK);
    b.extend_from_slice(&lo.to_le_bytes());
    b.extend_from_slice(&hi.to_le_bytes());
    for i in 0..81 {
        b.extend_from_slice(&[depth[i], 0, 0, 0]);
        b.extend_from_slice(&surface[i].to_le_bytes());
    }
    for (k, &w) in wet.iter().enumerate() {
        let (r, c) = (k / 8, k % 8);
        let deep = [
            r * 9 + c,
            r * 9 + c + 1,
            (r + 1) * 9 + c,
            (r + 1) * 9 + c + 1,
        ]
        .iter()
        .all(|&v| depth[v] >= DEEP_BYTE);
        b.push(match (w, deep) {
            (false, _) => CELL_DRY,
            (true, false) => CELL_RIVER,
            (true, true) => CELL_RIVER | 0x40,
        });
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for k in 0..20 {
        let v = if k % 10 < 3 { f32::MAX.to_bits() } else { 0 };
        b.extend_from_slice(&v.to_le_bytes());
    }
    b
}

/// The water of zone chunk `(gx, gy)`. Where two bodies wet one cell, the higher shows, whoever
/// made it; vertices no wet cell touches take the mean level of those that do.
pub fn chunk_water(z: &Zone, gx: usize, gy: usize) -> Option<Vec<u8>> {
    let o = [gx as f64 * CHUNK, gy as f64 * CHUNK];
    let h = &z.heights;
    let mut wet = [false; 64];
    let mut surface = [f32::NAN; 81];
    let mut bodies: Vec<_> = z.water.iter().collect();
    bodies.sort_by(|(a, wa), (b, wb)| wa.level.cmp(&wb.level).then(a.cmp(b)));
    for (_, w) in bodies {
        let [lo, hi] = w.shape.bounds();
        if hi[0] < o[0] || lo[0] > o[0] + CHUNK || hi[1] < o[1] || lo[1] > o[1] + CHUNK {
            continue;
        }
        let level = w.level as f64 / 100.0;
        for (k, cell) in wet.iter_mut().enumerate() {
            let (r, c) = (k / 8, k % 8);
            let middle = [
                o[0] + (c as f64 + 0.5) * CELL,
                o[1] + (r as f64 + 0.5) * CELL,
            ];
            if w.shape.reach(middle).is_none() {
                continue;
            }
            let (i, j) = (gx * 8 + c, gy * 8 + r);
            let low = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .map(|&(a, b)| f64::from(h.outer(i + a, j + b)))
                .fold(f64::MAX, f64::min);
            if low >= level {
                continue;
            }
            *cell = true;
            for (a, b) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                surface[(r + b) * 9 + c + a] = level as f32;
            }
        }
    }
    if !wet.iter().any(|&b| b) {
        return None;
    }
    let set: Vec<f32> = surface.iter().copied().filter(|v| !v.is_nan()).collect();
    let mean = set.iter().sum::<f32>() / set.len() as f32;
    let surface: [f32; 81] = std::array::from_fn(|i| {
        if surface[i].is_nan() {
            mean
        } else {
            surface[i]
        }
    });
    let ground: [f32; 81] = std::array::from_fn(|i| h.outer(gx * 8 + i % 9, gy * 8 + i / 9));
    Some(river(&surface, &ground, &wet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_river_block_follows_the_shipped_rules() {
        let surface = [50.0f32; 81];
        let mut ground = [49.5f32; 81];
        ground[0] = 40.0;
        ground[80] = 55.0;
        let mut wet = [false; 64];
        wet[0] = true;
        wet[63] = true;
        let b = river(&surface, &ground, &wet);
        assert_eq!(b.len(), BLOCK);
        let vertex = |i: usize| b[8 + i * 8];
        assert_eq!((vertex(0), vertex(1), vertex(80)), (90, 4, 0));
        let cells = &b[0x290..0x2d0];
        assert_eq!(
            (cells[0], cells[1], cells[63]),
            (CELL_RIVER, CELL_DRY, CELL_RIVER)
        );
        assert_eq!(
            f32::from_le_bytes([b[0], b[1], b[2], b[3]]).to_bits(),
            50f32.to_bits()
        );
        let deep = river(&surface, &[30.0; 81], &wet);
        assert_eq!(deep[0x290], CELL_RIVER | 0x40);
    }

    #[test]
    fn where_two_bodies_meet_the_higher_shows_whoever_made_it() {
        use crate::frame::Frame;
        use crate::shape::Shape;
        use crate::zone::{Id, Texture, Water};
        let mut z = Zone::new(
            "W",
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
        let body = |level| Water {
            level,
            shape: Shape::Rect {
                a: [0.0, 0.0],
                b: [30.0, 30.0],
            },
        };
        for (author, level) in [("ai", 5_000), ("sam", 4_000)] {
            let id = Id {
                author: author.into(),
                n: 1,
            };
            z.water.insert(id, body(level));
        }
        let b = chunk_water(&z, 0, 0).unwrap_or_default();
        let height = |i: usize| {
            f32::from_le_bytes([b[12 + i * 8], b[13 + i * 8], b[14 + i * 8], b[15 + i * 8]])
        };
        assert_eq!(height(0).to_bits(), 50f32.to_bits());
    }
}
