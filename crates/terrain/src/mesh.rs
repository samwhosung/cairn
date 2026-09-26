use adt::{CombinedAlphaMap, MCNK_DO_NOT_FIX_ALPHA, McnkChunk, McshChunk, RootAdt, parse_adt};

use crate::Error;
use crate::liquid::{LiquidMesh, build_liquid_mesh};

/// Edge length in texels of a chunk's combined alpha map.
pub const ALPHA_MAP_SIZE: u32 = 64;
/// Edge length in texels of a chunk's baked shadow map.
pub const SHADOW_MAP_SIZE: u32 = 64;
/// Yards per ADT tile, a 64th of a map's edge.
pub const TILE_SIZE: f32 = 533.333_3;
/// Yards per chunk; a tile is 16×16 chunks.
pub const CHUNK_SIZE: f32 = TILE_SIZE / 16.0;
pub(crate) const CELL_SIZE: f32 = CHUNK_SIZE / 8.0;
/// How many times a layer texture repeats across one chunk: once per cell.
pub const LAYER_REPEATS_PER_CHUNK: f32 = 8.0;

/// Entries in a chunk's vertex arrays: the 9×9 outer grid and the 8×8 cell centres.
pub const VERTICES: usize = 145;
const ROW_STRIDE: u32 = 17;
const MAP_CENTER: f32 = 32.0 * TILE_SIZE;
const MAP_CENTER_F64: f64 = 32.0 * 1600.0 / 3.0;
const CELL_SIZE_F64: f64 = (1600.0 / 3.0) / 128.0;

/// One MCNK chunk as an indexed triangle mesh in world coordinates (X north, Y west, Z up, in
/// yards), with what it needs for texturing.
///
/// The vertex arrays hold 145 entries in the MCVT order: outer vertex `(r, c)`, `r, c` in
/// `0..=8`, at `r * 17 + c`, and the inner vertex at the centre of cell `(r, c)`, `r, c` in
/// `0..=7`, at `r * 17 + 9 + c`. Rows step south and columns east. Each cell is four triangles
/// fanned from its centre: `(ctr, tl, bl), (ctr, bl, br), (ctr, br, tr), (ctr, tr, tl)`.
#[derive(Debug, Clone)]
pub struct ChunkMesh {
    pub positions: Vec<[f32; 3]>,
    /// Unit normals parallel to `positions`, or empty when the chunk has none.
    pub normals: Vec<[f32; 3]>,
    /// `0..1` across the chunk, parallel to `positions`: outer `(c/8, r/8)`, inner
    /// `((c+½)/8, (r+½)/8)`. Layer textures repeat [`LAYER_REPEATS_PER_CHUNK`] times over this.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list; the cells under a hole are left out, their vertices kept.
    pub indices: Vec<u32>,
    /// Bit `(row >> 1) * 4 + (col >> 1)` set: the 2×2 block of cells holding cell
    /// `(row, col)` is a hole.
    pub holes: u16,
    /// The first entry of `layer_textures`.
    pub base_texture: Option<String>,
    /// The texture of each layer, the base first. A layer whose texture index is out of range is
    /// dropped.
    pub layer_textures: Vec<String>,
    /// Parallel to `layer_textures`: each layer's `GroundEffectTexture` id.
    pub layer_effect_ids: Vec<u32>,
    /// RGBA, [`ALPHA_MAP_SIZE`] squared, row-major: R, G and B are the opacity of layers 1, 2
    /// and 3. `None` with fewer than two layers.
    pub alpha_map: Option<Vec<u8>>,
    /// [`SHADOW_MAP_SIZE`] squared, row-major, 255 in shadow and 0 lit. `None` without MCSH.
    pub shadow: Option<Vec<u8>>,
    /// Per cell, `row * 8 + col`: the layer whose ground effect the cell uses.
    pub pred_tex: [u8; 64],
    /// Per cell, `row * 8 + col`: the cell gets no ground-effect doodads.
    pub no_effect_doodad: [bool; 64],
    /// The chunk's column in its tile's 16×16 grid.
    pub index_x: u32,
    /// The chunk's row in its tile's 16×16 grid.
    pub index_y: u32,
    /// The chunk's `AreaTable` id.
    pub area_id: u32,
    /// The client fences the whole chunk against movers entering it.
    pub impassable: bool,
    /// One surface per MCLQ block that meshes.
    pub liquids: Vec<LiquidMesh>,
}

/// A placed M2 doodad.
#[derive(Debug, Clone)]
pub struct Doodad {
    /// The model path as the ADT names it, ending `.mdx`.
    pub model: String,
    /// World coordinates.
    pub position: [f32; 3],
    /// Euler angles in degrees; Y is the heading.
    pub rotation: [f32; 3],
    pub scale: f32,
    /// The same on every tile the doodad overlaps.
    pub unique_id: u32,
}

/// A placed WMO.
#[derive(Debug, Clone)]
pub struct WmoInstance {
    /// The root WMO path.
    pub model: String,
    /// World coordinates.
    pub position: [f32; 3],
    /// Euler angles in degrees; Y is the heading.
    pub rotation: [f32; 3],
    /// The same on every tile the building overlaps.
    pub unique_id: u32,
    /// The doodad set shown in addition to set 0.
    pub doodad_set: u16,
    /// The `WMOAreaTable` name set.
    pub name_set: u16,
}

/// One ADT tile: its chunks in file order, and its placements.
#[derive(Debug, Clone)]
pub struct TileMesh {
    pub chunks: Vec<ChunkMesh>,
    pub doodads: Vec<Doodad>,
    pub wmos: Vec<WmoInstance>,
}

impl TileMesh {
    /// Terrain vertices across all chunks; liquids are not counted.
    pub fn vertex_count(&self) -> usize {
        self.chunks.iter().map(|c| c.positions.len()).sum()
    }
}

/// Snaps a world X or Y to the lattice of cell corners. Each chunk stores its corner as an `f32`,
/// so the same edge vertex computed from two neighbouring chunks can differ by a rounding error;
/// snapping makes them one point.
pub(crate) fn snap_to_corner_lattice(coord: f32) -> f32 {
    let idx = ((MAP_CENTER_F64 - f64::from(coord)) / CELL_SIZE_F64).round();
    (MAP_CENTER_F64 - idx * CELL_SIZE_F64) as f32
}

/// Builds a tile from an ADT's bytes. Chunks without a full height grid are skipped; the tile
/// fails when the ADT does not parse or no chunk has one.
pub fn adt_to_tile_mesh(adt_bytes: &[u8]) -> Result<TileMesh, Error> {
    let root = parse_adt(adt_bytes)?;
    let chunks: Vec<ChunkMesh> = root
        .mcnk_chunks
        .iter()
        .filter_map(|mcnk| chunk_mesh(&root, mcnk))
        .collect();
    if chunks.is_empty() {
        return Err(Error::NoTerrain);
    }
    let doodads = root
        .doodad_placements
        .iter()
        .filter_map(|d| {
            Some(Doodad {
                model: root.models.get(d.name_id as usize)?.clone(),
                position: placement_to_world(d.position),
                rotation: d.rotation,
                scale: f32::from(d.scale) / 1024.0,
                unique_id: d.unique_id,
            })
        })
        .collect();
    let wmos = root
        .wmo_placements
        .iter()
        .filter_map(|w| {
            Some(WmoInstance {
                model: root.wmos.get(w.name_id as usize)?.clone(),
                position: placement_to_world(w.position),
                rotation: w.rotation,
                unique_id: w.unique_id,
                doodad_set: w.doodad_set,
                name_set: w.name_set,
            })
        })
        .collect();
    Ok(TileMesh {
        chunks,
        doodads,
        wmos,
    })
}

/// Placements are stored as distances from the map's north-west corner, axes permuted.
fn placement_to_world(p: [f32; 3]) -> [f32; 3] {
    [MAP_CENTER - p[2], MAP_CENTER - p[0], p[1]]
}

fn chunk_mesh(root: &RootAdt, mcnk: &McnkChunk) -> Option<ChunkMesh> {
    let heights = &mcnk.heights.as_ref()?.heights;
    if heights.len() < VERTICES {
        return None;
    }
    let header = &mcnk.header;
    let [wx, wy, wz] = header.position;
    let mcnr = mcnk
        .normals
        .as_ref()
        .filter(|n| n.normals.len() >= VERTICES);

    let mut positions = Vec::with_capacity(VERTICES);
    let mut normals = Vec::with_capacity(if mcnr.is_some() { VERTICES } else { 0 });
    let mut uvs = Vec::with_capacity(VERTICES);
    let mut push = |idx: u32, position: [f32; 3], uv: [f32; 2]| {
        positions.push(position);
        if let Some(mcnr) = mcnr {
            normals.push(mcnr.normals[idx as usize].to_normalized());
        }
        uvs.push(uv);
    };
    for row in 0..9u32 {
        for col in 0..9u32 {
            let idx = row * ROW_STRIDE + col;
            let position = [
                snap_to_corner_lattice(wx - row as f32 * CELL_SIZE),
                snap_to_corner_lattice(wy - col as f32 * CELL_SIZE),
                wz + heights[idx as usize],
            ];
            push(idx, position, [col as f32 / 8.0, row as f32 / 8.0]);
        }
        if row == 8 {
            break;
        }
        for col in 0..8u32 {
            let idx = row * ROW_STRIDE + 9 + col;
            let position = [
                wx - (row as f32 * CELL_SIZE + CELL_SIZE / 2.0),
                wy - (col as f32 * CELL_SIZE + CELL_SIZE / 2.0),
                wz + heights[idx as usize],
            ];
            let uv = [(col as f32 + 0.5) / 8.0, (row as f32 + 0.5) / 8.0];
            push(idx, position, uv);
        }
    }

    let holes = header.holes_low_res;
    let mut indices = Vec::with_capacity(8 * 8 * 12);
    for row in 0..8u32 {
        for col in 0..8u32 {
            if is_hole(holes, row, col) {
                continue;
            }
            let [tl, tr, bl, br, ctr] = cell_vertices(row, col);
            indices.extend_from_slice(&[ctr, tl, bl, ctr, bl, br, ctr, br, tr, ctr, tr, tl]);
        }
    }

    let (layer_textures, layer_effect_ids): (Vec<String>, Vec<u32>) = mcnk
        .layers
        .iter()
        .flat_map(|mcly| &mcly.layers)
        .filter_map(|layer| {
            let texture = root.textures.get(layer.texture_id as usize)?;
            Some((texture.clone(), layer.effect_id))
        })
        .unzip();
    let base_texture = layer_textures.first().cloned();
    let fix_alpha = header.flags & MCNK_DO_NOT_FIX_ALPHA == 0;
    let alpha_map = (layer_textures.len() > 1).then(|| {
        CombinedAlphaMap::new(mcnk, false, fix_alpha)
            .as_slice()
            .to_vec()
    });
    let shadow = mcnk.shadow.as_ref().map(shadow_texels);
    let pred_tex = std::array::from_fn(|k| (header.pred_tex[k / 4] >> (2 * (k % 4))) & 0x3);
    let no_effect_doodad =
        std::array::from_fn(|k| (header.no_effect_doodad[k / 8] >> (k % 8)) & 0x1 == 1);
    let liquids = mcnk
        .liquids
        .iter()
        .filter_map(|mclq| build_liquid_mesh(mclq, header.position))
        .collect();

    Some(ChunkMesh {
        positions,
        normals,
        uvs,
        indices,
        holes,
        base_texture,
        layer_textures,
        layer_effect_ids,
        alpha_map,
        shadow,
        pred_tex,
        no_effect_doodad,
        index_x: header.index_x,
        index_y: header.index_y,
        area_id: header.area_id,
        impassable: header.impassable(),
        liquids,
    })
}

fn shadow_texels(mcsh: &McshChunk) -> Vec<u8> {
    let n = SHADOW_MAP_SIZE as usize;
    let mut out = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            if mcsh.is_shadowed(x, y) {
                out[y * n + x] = 255;
            }
        }
    }
    out
}

/// Whether cell `(row, col)` of a chunk with these hole bits is a hole.
pub fn is_hole(holes: u16, row: u32, col: u32) -> bool {
    holes & (1u16 << ((row >> 1) * 4 + (col >> 1))) != 0
}

/// Cell `(row, col)`'s corners and centre: `[tl, tr, bl, br, ctr]`.
pub fn cell_vertices(row: u32, col: u32) -> [u32; 5] {
    let tl = row * ROW_STRIDE + col;
    let bl = tl + ROW_STRIDE;
    [tl, tl + 1, bl, bl + 1, row * ROW_STRIDE + 9 + col]
}
