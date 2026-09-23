use crate::{INNER_EDGE, OUTER_EDGE, OUTER_N, WdlFile};

const TILE_SIZE: f32 = 533.333_3;
const CHUNK_SIZE: f32 = TILE_SIZE / 16.0;
const MAP_OFFSET: f64 = 32.0 * TILE_SIZE as f64;

/// A WDL tile as a triangle mesh in world coordinates (X north, Y west, Z up, in yards).
pub struct WdlTileMesh {
    /// The 17×17 chunk corners row by row, then the 16×16 chunk centres.
    pub positions: Vec<[f32; 3]>,
    /// Each chunk as four triangles fanned from its centre, wound counter-clockwise from above.
    pub indices: Vec<u32>,
}

impl WdlFile {
    /// Tile `(tile_x, tile_y)` meshed, or `None` where the map has no heights. Corners come from
    /// the map-wide chunk lattice, so neighbouring tiles share their edge vertices bit for bit.
    pub fn tile_mesh(&self, tile_x: u32, tile_y: u32) -> Option<WdlTileMesh> {
        let heights = self.heights(tile_x, tile_y)?;
        let lattice = |index: f64| (MAP_OFFSET - index * f64::from(CHUNK_SIZE)) as f32;
        let (row0, col0) = (f64::from(tile_y) * 16.0, f64::from(tile_x) * 16.0);
        let mut positions = Vec::with_capacity(OUTER_N + INNER_EDGE * INNER_EDGE);
        for r in 0..OUTER_EDGE {
            for c in 0..OUTER_EDGE {
                positions.push([
                    lattice(row0 + r as f64),
                    lattice(col0 + c as f64),
                    f32::from(heights.corners[r * OUTER_EDGE + c]),
                ]);
            }
        }
        for r in 0..INNER_EDGE {
            for c in 0..INNER_EDGE {
                positions.push([
                    lattice(row0 + r as f64 + 0.5),
                    lattice(col0 + c as f64 + 0.5),
                    f32::from(heights.centres[r * INNER_EDGE + c]),
                ]);
            }
        }
        let mut indices = Vec::with_capacity(INNER_EDGE * INNER_EDGE * 12);
        for r in 0..INNER_EDGE as u32 {
            for c in 0..INNER_EDGE as u32 {
                let tl = r * OUTER_EDGE as u32 + c;
                let (tr, bl) = (tl + 1, tl + OUTER_EDGE as u32);
                let br = bl + 1;
                let ctr = OUTER_N as u32 + r * INNER_EDGE as u32 + c;
                indices.extend_from_slice(&[ctr, tl, bl, ctr, bl, br, ctr, br, tr, ctr, tr, tl]);
            }
        }
        Some(WdlTileMesh { positions, indices })
    }
}
