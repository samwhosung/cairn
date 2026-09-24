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

    /// The height of the meshed surface over world `(x, y)`, or `None` where the map has no
    /// heights.
    pub fn height_at(&self, world_x: f32, world_y: f32) -> Option<f32> {
        let cs = f64::from(CHUNK_SIZE);
        let gr = (MAP_OFFSET - f64::from(world_x)) / cs;
        let gc = (MAP_OFFSET - f64::from(world_y)) / cs;
        if !(0.0..1024.0).contains(&gr) || !(0.0..1024.0).contains(&gc) {
            return None;
        }
        let (cell_r, cell_c) = (gr as usize, gc as usize);
        let tile = self.heights((cell_c / 16) as u32, (cell_r / 16) as u32)?;
        let (r, c) = (cell_r % 16, cell_c % 16);
        let h = |rr: usize, cc: usize| f64::from(tile.corners[rr * OUTER_EDGE + cc]);
        let (tl, tr, bl, br) = (h(r, c), h(r, c + 1), h(r + 1, c), h(r + 1, c + 1));
        let ctr = f64::from(tile.centres[r * INNER_EDGE + c]);
        let (v, u) = (gr - cell_r as f64, gc - cell_c as f64);
        let (edge_from, edge_to, along, inward) = if v <= u && v <= 1.0 - u {
            (tl, tr, u, v)
        } else if v >= u && v >= 1.0 - u {
            (bl, br, u, 1.0 - v)
        } else if u < v {
            (tl, bl, v, u)
        } else {
            (tr, br, v, 1.0 - u)
        };
        let height = if inward >= 0.5 {
            ctr
        } else {
            let t = (along - inward) / (1.0 - 2.0 * inward);
            let edge = edge_from + (edge_to - edge_from) * t;
            edge + (ctr - edge) * (inward * 2.0)
        };
        Some(height as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Heights, TILES_PER_MAP, maof_index};

    #[test]
    fn heights_lie_on_the_mesh() {
        let (tx, ty) = (30, 41);
        let mut tiles: Vec<Option<Heights>> =
            (0..TILES_PER_MAP * TILES_PER_MAP).map(|_| None).collect();
        tiles[maof_index(tx, ty)] = Some(Heights {
            corners: std::array::from_fn(|i| ((i * 37) % 251) as i16 - 100),
            centres: std::array::from_fn(|i| ((i * 53) % 211) as i16 - 60),
        });
        let wdl = WdlFile { tiles };
        let mesh = wdl
            .tile_mesh(tx as u32, ty as u32)
            .expect("the tile is there");
        for (i, p) in mesh.positions.iter().enumerate() {
            let (r, c) = (i / OUTER_EDGE, i % OUTER_EDGE);
            if i < OUTER_N && (r == OUTER_EDGE - 1 || c == OUTER_EDGE - 1) {
                continue;
            }
            let h = wdl.height_at(p[0], p[1]).expect("on the tile");
            assert!((h - p[2]).abs() < 0.01, "vertex {i}: {h} vs {}", p[2]);
        }
        assert_eq!(wdl.height_at(0.0, 0.0), None);
    }
}
