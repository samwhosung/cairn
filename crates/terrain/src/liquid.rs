use adt::MclqChunk;

use crate::mesh::{CELL_SIZE, snap_to_corner_lattice};

/// Which texture set and render path a liquid surface uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    /// River and lake water.
    Still,
    /// Fast river water. No shipped 1.12.1 liquid selects it.
    Rapids,
    Ocean,
    /// Lava: opaque and unlit, but fogged.
    Magma,
    /// Opaque and unlit like magma. The client draws it only from WMO liquid.
    Slime,
}

impl LiquidKind {
    /// `None` for no liquid.
    pub fn from_wmo_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 0xf {
            0 | 4 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 | 6 => Some(LiquidKind::Magma),
            3 | 7 => Some(LiquidKind::Slime),
            8 => Some(LiquidKind::Rapids),
            _ => None,
        }
    }

    /// The kind an ADT liquid cell's type nibble selects. The client draws ADT liquid in three
    /// queues keyed by `nibble & 3` and has none for slime, so class 3 is `None`.
    pub fn from_adt_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 3 {
            0 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 => Some(LiquidKind::Magma),
            _ => None,
        }
    }

    /// Whether the surface is drawn opaque and unlit: its texture is the whole colour.
    pub fn is_fullbright(self) -> bool {
        matches!(self, LiquidKind::Magma | LiquidKind::Slime)
    }
}

/// One liquid surface as a regular vertex grid in world coordinates, with its triangle list.
///
/// The per-vertex arrays are the grid, row-major. `indices` holds two triangles per wet cell; the
/// vertices under dry cells stay, unreferenced. The client samples height bilinearly from a cell's
/// four corners, not from these triangles.
#[derive(Debug, Clone)]
pub struct LiquidMesh {
    /// Vertex counts `[cols, rows]`; `[9, 9]` for an ADT chunk.
    pub grid: [u32; 2],
    /// Per cell, row-major over `(cols − 1) × (rows − 1)`: the cell holds liquid. A liquid's
    /// bounding box can span dry ground, so containment is this, not the box.
    pub wet: Vec<bool>,
    /// Per cell, parallel to `wet`: a neighbouring WMO group claims the cell too, and only one
    /// of the two may draw it. Always `false` for ADT liquid.
    pub shared: Vec<bool>,
    /// A vertex with no real height takes the block's minimum height.
    pub positions: Vec<[f32; 3]>,
    /// Water and ocean: a quarter per cell. ADT magma: the coordinates authored per vertex.
    pub uvs: Vec<[f32; 2]>,
    /// Per vertex, the `v` coordinate into the depth swatch. Zero, and unused, for magma.
    pub depths: Vec<f32>,
    pub indices: Vec<u32>,
    /// The majority type nibble of the wet cells. The ambient sound is keyed by it, and it tells
    /// apart river speeds the render kind does not.
    pub sound_nibble: u8,
    /// A WMO liquid's index into its root's materials, whose diffuse colour it takes indoors.
    /// `None` for ADT liquid.
    pub material_id: Option<u16>,
    pub kind: LiquidKind,
}

const GRID: usize = 9;
const CELLS: usize = 8;
const DRY_NIBBLE: u8 = 0x0f;
const MAGMA_TEXCOORD_SCALE: f32 = 3.0 / 256.0;
/// A vertex under no liquid can carry `f32::MAX`, which is finite; heights at or past this are
/// not real.
const MAX_REAL_HEIGHT: f32 = 1.0e9;
/// The depth bytes at which the swatch coordinate reaches 1: the client ramps river and ocean
/// depths separately.
const RIVER_DEPTH_V_SATURATION: f32 = 42.0;
const OCEAN_DEPTH_V_SATURATION: f32 = 255.0;

fn is_real_height(h: f32) -> bool {
    h.is_finite() && h.abs() < MAX_REAL_HEIGHT
}

/// Builds the surface of one MCLQ block in the chunk whose header position is `position`.
///
/// `None` when no cell is wet, or when the block is slime. The kind is the majority nibble of
/// the wet cells, not the MCNK header's liquid bits.
pub fn build_liquid_mesh(mclq: &MclqChunk, position: [f32; 3]) -> Option<LiquidMesh> {
    if mclq.vertices.len() < GRID * GRID || mclq.tile_flags.len() < CELLS * CELLS {
        return None;
    }

    let mut indices = Vec::with_capacity(CELLS * CELLS * 6);
    let mut wet = vec![false; CELLS * CELLS];
    let mut nibble_counts = [0u32; 16];
    for row in 0..CELLS {
        for col in 0..CELLS {
            let nibble = mclq.tile_flags[row * CELLS + col] & 0x0f;
            if nibble == DRY_NIBBLE {
                continue;
            }
            let tl = (row * GRID + col) as u32;
            let tr = tl + 1;
            let bl = ((row + 1) * GRID + col) as u32;
            let br = bl + 1;
            if [tl, tr, bl, br]
                .iter()
                .any(|&i| !is_real_height(mclq.vertices[i as usize].height))
            {
                continue;
            }
            nibble_counts[nibble as usize] += 1;
            wet[row * CELLS + col] = true;
            indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
        }
    }
    if indices.is_empty() {
        return None;
    }

    let sound_nibble = nibble_counts
        .iter()
        .enumerate()
        .max_by_key(|&(n, &c)| (c, n))
        .map_or(0, |(n, _)| n as u8);
    let kind = LiquidKind::from_adt_nibble(sound_nibble)?;
    let depth_v_div = match kind {
        LiquidKind::Ocean => OCEAN_DEPTH_V_SATURATION,
        _ => RIVER_DEPTH_V_SATURATION,
    };
    let [wx, wy, _] = position;

    let mut positions = Vec::with_capacity(GRID * GRID);
    let mut uvs = Vec::with_capacity(GRID * GRID);
    let mut depths = Vec::with_capacity(GRID * GRID);
    for (n, vertex) in mclq.vertices[..GRID * GRID].iter().enumerate() {
        let row = (n / GRID) as f32;
        let col = (n % GRID) as f32;
        let h = if is_real_height(vertex.height) {
            vertex.height
        } else {
            mclq.min_height
        };
        positions.push([
            snap_to_corner_lattice(wx - row * CELL_SIZE),
            snap_to_corner_lattice(wy - col * CELL_SIZE),
            h,
        ]);
        if kind == LiquidKind::Magma {
            let [s, t] = vertex.texcoords();
            let scale = f64::from(MAGMA_TEXCOORD_SCALE);
            uvs.push([(f64::from(s) * scale) as f32, (f64::from(t) * scale) as f32]);
            depths.push(0.0);
        } else {
            uvs.push([col * 0.25, row * 0.25]);
            depths.push((f32::from(vertex.depth_byte()) / depth_v_div).clamp(0.0, 1.0));
        }
    }

    Some(LiquidMesh {
        grid: [GRID as u32, GRID as u32],
        shared: vec![false; wet.len()],
        wet,
        positions,
        uvs,
        depths,
        indices,
        sound_nibble,
        material_id: None,
        kind,
    })
}

impl LiquidMesh {
    /// Dries the wet cells `keep` rejects, by cell index as in `wet`, and rebuilds `indices`.
    pub fn retain_cells(&mut self, keep: impl Fn(usize) -> bool) {
        let (cols, rows) = (self.grid[0] as usize, self.grid[1] as usize);
        let (xt, yt) = (cols.saturating_sub(1), rows.saturating_sub(1));
        let mut dropped = false;
        for (c, wet) in self.wet.iter_mut().enumerate() {
            if *wet && !keep(c) {
                *wet = false;
                dropped = true;
            }
        }
        if !dropped {
            return;
        }
        self.indices.clear();
        for ty in 0..yt {
            for tx in 0..xt {
                if !self.wet[ty * xt + tx] {
                    continue;
                }
                let tl = (ty * cols + tx) as u32;
                let (tr, bl) = (tl + 1, ((ty + 1) * cols + tx) as u32);
                let br = bl + 1;
                self.indices.extend_from_slice(&[tl, bl, br, tl, br, tr]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use adt::LiquidVertex;

    use super::*;

    fn block(height: impl Fn(usize) -> f32, flags: impl Fn(usize) -> u8) -> MclqChunk {
        MclqChunk {
            min_height: -1.0,
            max_height: 1.0,
            vertices: (0..GRID * GRID)
                .map(|n| LiquidVertex {
                    union_data: [84, 0, 0, 1],
                    height: height(n),
                })
                .collect(),
            tile_flags: std::array::from_fn(flags),
        }
    }

    #[test]
    fn dry_cells_and_sentinel_corners_carry_no_triangles() {
        let mclq = block(
            |n| if n == 80 { f32::MAX } else { 5.0 },
            |c| if c == 0 { DRY_NIBBLE } else { 0 },
        );
        let mesh = build_liquid_mesh(&mclq, [0.0, 0.0, 0.0]).expect("wet");
        assert!(!mesh.wet[0] && !mesh.wet[63]);
        assert_eq!(mesh.wet.iter().filter(|w| **w).count(), 62);
        assert_eq!(mesh.indices.len(), 62 * 6);
        assert_eq!(mesh.positions[80][2].to_bits(), (-1.0f32).to_bits());
        assert_eq!(mesh.kind, LiquidKind::Still);
        assert_eq!(mesh.depths[0].to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn the_majority_nibble_decides_and_slime_builds_nothing() {
        let ocean = block(|_| 0.0, |c| if c < 40 { 1 } else { 4 });
        let mesh = build_liquid_mesh(&ocean, [0.0; 3]).expect("wet");
        assert_eq!((mesh.kind, mesh.sound_nibble), (LiquidKind::Ocean, 1));
        assert_eq!(mesh.depths[0].to_bits(), (84.0f32 / 255.0).to_bits());
        let tie = block(|_| 0.0, |c| if c < 32 { 1 } else { 4 });
        let mesh = build_liquid_mesh(&tie, [0.0; 3]).expect("wet");
        assert_eq!((mesh.kind, mesh.sound_nibble), (LiquidKind::Still, 4));
        let slime = block(|_| 0.0, |_| 3);
        assert!(build_liquid_mesh(&slime, [0.0; 3]).is_none());
        let dry = block(|_| 0.0, |_| DRY_NIBBLE);
        assert!(build_liquid_mesh(&dry, [0.0; 3]).is_none());
    }

    #[test]
    fn magma_reads_its_uvs_from_the_vertex() {
        let magma = block(|_| 0.0, |_| 6);
        let mesh = build_liquid_mesh(&magma, [0.0; 3]).expect("wet");
        assert_eq!(mesh.kind, LiquidKind::Magma);
        let u = (84.0f64 * 3.0 / 256.0) as f32;
        let v = (256.0f64 * 3.0 / 256.0) as f32;
        assert_eq!(mesh.uvs[0].map(f32::to_bits), [u.to_bits(), v.to_bits()]);
        assert_eq!(mesh.depths[0].to_bits(), 0);
    }

    #[test]
    fn the_kind_tables_cover_every_nibble() {
        let wmo: Vec<Option<LiquidKind>> = (0..16).map(LiquidKind::from_wmo_nibble).collect();
        assert_eq!(wmo[5], None);
        assert_eq!(wmo[8], Some(LiquidKind::Rapids));
        assert!(wmo[9..].iter().all(Option::is_none));
        assert_eq!(LiquidKind::from_adt_nibble(8), Some(LiquidKind::Still));
        assert_eq!(LiquidKind::from_adt_nibble(7), None);
        assert!(LiquidKind::Slime.is_fullbright() && !LiquidKind::Ocean.is_fullbright());
    }

    #[test]
    fn retain_cells_rebuilds_the_triangles() {
        let mut mesh = build_liquid_mesh(&block(|_| 0.0, |_| 0), [0.0; 3]).expect("wet");
        mesh.retain_cells(|c| c % 2 == 0);
        assert_eq!(mesh.wet.iter().filter(|w| **w).count(), 32);
        assert_eq!(mesh.indices.len(), 32 * 6);
        assert_eq!(&mesh.indices[..6], &[0, 9, 10, 0, 10, 1]);
    }
}
