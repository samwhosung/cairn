use crate::mesh::{
    CHUNK_SIZE, ChunkMesh, SHADOW_MAP_SIZE, TILE_SIZE, VERTICES, cell_vertices, is_hole,
};

const CHUNKS_PER_TILE: usize = 256;
const CHUNKS_PER_ROW: f32 = 16.0;

impl ChunkMesh {
    fn fraction_south_east(&self, point: [f32; 3]) -> Option<(f32, f32)> {
        let nw = *self.positions.first()?;
        let south = (nw[0] - point[0]) / TILE_SIZE * 16.0;
        let east = (nw[1] - point[1]) / TILE_SIZE * 16.0;
        ((0.0..=1.0).contains(&south) && (0.0..=1.0).contains(&east)).then_some((south, east))
    }

    /// Whether the baked shadow covers `point`; a chunk without one is lit. `None` outside the
    /// chunk; its edges count as inside.
    pub fn mcsh_shadowed_at(&self, point: [f32; 3]) -> Option<bool> {
        let (south, east) = self.fraction_south_east(point)?;
        let Some(shadow) = self.shadow.as_ref() else {
            return Some(false);
        };
        let n = SHADOW_MAP_SIZE as usize;
        let row = ((south * n as f32) as usize).min(n - 1);
        let col = ((east * n as f32) as usize).min(n - 1);
        Some(shadow.get(row * n + col).copied().unwrap_or(0) >= 128)
    }

    /// The `GroundEffectTexture` id of the cell under `point`, `None` also when the cell's layer
    /// is missing or its effect id is `u32::MAX`.
    pub fn ground_effect_at(&self, point: [f32; 3]) -> Option<u32> {
        let (south, east) = self.fraction_south_east(point)?;
        let row = ((south * 8.0) as usize).min(7);
        let col = ((east * 8.0) as usize).min(7);
        let layer = self.pred_tex[row * 8 + col] as usize;
        let effect = self.layer_effect_ids.get(layer).copied()?;
        (effect != u32::MAX).then_some(effect)
    }

    /// The chunk's `AreaTable` id, when `point` is over it.
    pub fn area_at(&self, point: [f32; 3]) -> Option<u32> {
        self.fraction_south_east(point).map(|_| self.area_id)
    }

    /// Whether the chunk is impassable, when `point` is over it.
    pub fn impassable_at(&self, point: [f32; 3]) -> Option<bool> {
        self.fraction_south_east(point).map(|_| self.impassable)
    }

    /// The surface height under `point`'s column, `None` also over a hole.
    pub fn height_at(&self, point: [f32; 3]) -> Option<f32> {
        let (south, east) = self.fraction_south_east(point)?;
        if self.positions.len() == VERTICES {
            let row = ((south * 8.0) as u32).min(7);
            let col = ((east * 8.0) as u32).min(7);
            if is_hole(self.holes, row, col) {
                return None;
            }
            let [tl, tr, bl, br, ctr] = cell_vertices(row, col).map(|i| self.positions[i as usize]);
            return [[ctr, tl, bl], [ctr, bl, br], [ctr, br, tr], [ctr, tr, tl]]
                .iter()
                .find_map(|tri| triangle_z_at(tri, point[0], point[1]));
        }
        self.indices.as_chunks::<3>().0.iter().find_map(|t| {
            let tri = [
                *self.positions.get(t[0] as usize)?,
                *self.positions.get(t[1] as usize)?,
                *self.positions.get(t[2] as usize)?,
            ];
            triangle_z_at(&tri, point[0], point[1])
        })
    }
}

/// The height of `tri` over column `(x, y)`, `None` outside its XY projection or when it is
/// vertical.
pub fn triangle_z_at(tri: &[[f32; 3]; 3], x: f32, y: f32) -> Option<f32> {
    let [a, b, c] = *tri;
    let det = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if det.abs() < 1.0e-9 {
        return None;
    }
    let l1 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / det;
    let l2 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / det;
    let l3 = 1.0 - l1 - l2;
    if l1 < 0.0 || l2 < 0.0 || l3 < 0.0 {
        return None;
    }
    Some(l1 * a[2] + l2 * b[2] + l3 * c[2])
}

/// The `AreaTable` id under `point`, given one tile's chunks. `None` outside them.
pub fn area_id_at(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<u32> {
    chunks.iter().find_map(|c| c.area_at(point))
}

/// Whether the chunk under `point` is impassable, given one tile's chunks. `None` outside them.
pub fn impassable_at(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<bool> {
    chunks.iter().find_map(|c| c.impassable_at(point))
}

/// The `GroundEffectTexture` id under `point`, given one tile's chunks. `None` outside them, or
/// where the cell's layer is missing or its effect id is `u32::MAX`.
pub fn ground_effect_at(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<u32> {
    chunks.iter().find_map(|c| c.ground_effect_at(point))
}

/// The surface height under `point`'s column, given one tile's chunks. `None` outside them or over
/// a hole.
pub fn terrain_height_at(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<f32> {
    match chunk_in_full_tile(chunks, point) {
        Some(c) => c.height_at(point),
        None => chunks.iter().find_map(|c| c.height_at(point)),
    }
}

/// Whether the baked shadow covers `point`, given one tile's chunks. `None` outside them.
pub fn mcsh_shadowed_at(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<bool> {
    match chunk_in_full_tile(chunks, point) {
        Some(c) => c.mcsh_shadowed_at(point),
        None => chunks.iter().find_map(|c| c.mcsh_shadowed_at(point)),
    }
}

fn chunk_in_full_tile(chunks: &[ChunkMesh], point: [f32; 3]) -> Option<&ChunkMesh> {
    if chunks.len() != CHUNKS_PER_TILE {
        return None;
    }
    let nw = *chunks[0].positions.first()?;
    let row = (nw[0] - point[0]) / CHUNK_SIZE;
    let col = (nw[1] - point[1]) / CHUNK_SIZE;
    if !(0.0..CHUNKS_PER_ROW).contains(&row) || !(0.0..CHUNKS_PER_ROW).contains(&col) {
        return None;
    }
    chunks.get(row as usize * 16 + col as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> ChunkMesh {
        ChunkMesh {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            holes: 0,
            base_texture: None,
            layer_textures: Vec::new(),
            layer_effect_ids: Vec::new(),
            alpha_map: None,
            shadow: None,
            pred_tex: [0; 64],
            no_effect_doodad: [false; 64],
            index_x: 0,
            index_y: 0,
            area_id: 0,
            impassable: false,
            liquids: Vec::new(),
        }
    }

    #[test]
    fn height_at_interpolates_inside_and_misses_holes_and_off_footprint() {
        let mid_x = -CHUNK_SIZE * 0.5;
        let far_x = -CHUNK_SIZE;
        let far_y = -CHUNK_SIZE;
        let ridge_z = 10.0;
        let positions = vec![
            [0.0, 0.0, 0.0],
            [0.0, far_y, 0.0],
            [mid_x, 0.0, ridge_z],
            [mid_x, far_y, ridge_z],
            [far_x, 0.0, 0.0],
            [far_x, far_y, 0.0],
        ];
        let north_half_only = vec![0, 1, 2, 1, 3, 2];
        let c = chunk(positions, north_half_only);

        let quarter_x = -CHUNK_SIZE * 0.25;
        let z = c
            .height_at([quarter_x, -CHUNK_SIZE * 0.5, 0.0])
            .unwrap_or(f32::NAN);
        assert!((z - 5.0).abs() < 0.01, "halfway up the ridge, got {z}");
        assert_eq!(c.height_at([far_x + 1.0, -CHUNK_SIZE * 0.5, 0.0]), None);
        assert_eq!(c.height_at([quarter_x, far_y - 5.0, 0.0]), None);
    }

    #[test]
    fn triangle_z_at_rejects_vertical_and_outside() {
        let flat = [[0.0, 0.0, 2.0], [1.0, 0.0, 2.0], [0.0, 1.0, 2.0]];
        assert_eq!(
            triangle_z_at(&flat, 0.25, 0.25).map(f32::to_bits),
            Some(2.0f32.to_bits())
        );
        assert_eq!(triangle_z_at(&flat, 0.75, 0.75), None);
        let vertical = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.0, 5.0]];
        assert_eq!(triangle_z_at(&vertical, 0.5, 0.0), None);
    }

    #[test]
    fn a_full_tile_answers_by_lookup_like_the_walk() {
        let mut chunks = Vec::new();
        for row in 0..16 {
            for col in 0..16 {
                let (x, y) = (-(row as f32) * CHUNK_SIZE, -(col as f32) * CHUNK_SIZE);
                let mut c = chunk(vec![[x, y, 0.0]], Vec::new());
                c.area_id = row * 16 + col;
                chunks.push(c);
            }
        }
        for (i, j) in [(0.5, 0.5), (3.2, 11.9), (15.9, 0.1), (7.5, 2.25)] {
            let point = [-i * CHUNK_SIZE, -j * CHUNK_SIZE, 0.0];
            let area = area_id_at(&chunks, point).expect("inside");
            assert_eq!(area, (i as u32) * 16 + j as u32, "{point:?}");
            assert_eq!(
                chunk_in_full_tile(&chunks, point).map(|c| c.area_id),
                Some(area)
            );
        }
        assert!(chunk_in_full_tile(&chunks, [1.0, 0.0, 0.0]).is_none());
        assert!(chunk_in_full_tile(&chunks[1..], [-1.0, -1.0, 0.0]).is_none());
    }
}
