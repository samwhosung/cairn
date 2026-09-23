use wowfile::{ByteExt, chunks};

use crate::Error;
use crate::liquid::{WmoLiquid, parse_mliq};
use crate::record::{f32_le, u16_le, u32_le, whole_records};

/// One group file's geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct WmoGroup {
    /// `MOGP` flags. The client lights and draws the group as exterior when `flags & 0x48` is set.
    pub flags: u32,
    /// The whole group's liquid type, or `0xf` when each `MLIQ` tile names its own.
    pub group_liquid: u32,
    pub vertex_positions: Vec<Vec3>,
    pub vertex_normals: Vec<Vec3>,
    pub texture_coords: Vec<Uv>,
    pub vertex_colors: Vec<Color>,
    pub vertex_indices: Vec<u16>,
    pub render_batches: Vec<Batch>,
    /// One entry per triangle.
    pub material_info: Vec<MopyEntry>,
    pub liquid: Option<WmoLiquid>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uv {
    pub u: f32,
    pub v: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

/// One `MOBA` render batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Batch {
    pub start_index: u32,
    pub count: u16,
    pub material_id: u8,
}

/// One `MOPY` entry: a triangle's flags and material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MopyEntry {
    pub flags: u8,
    pub material_id: u8,
}

const MOGP_HEADER_SIZE: usize = 68;

pub(crate) fn parse_group(mogp: &[u8]) -> Result<WmoGroup, Error> {
    let mut g = WmoGroup {
        flags: mogp.u32_at(8).unwrap_or(0),
        group_liquid: mogp.u32_at(0x34).unwrap_or(0xf),
        vertex_positions: Vec::new(),
        vertex_normals: Vec::new(),
        texture_coords: Vec::new(),
        vertex_colors: Vec::new(),
        vertex_indices: Vec::new(),
        render_batches: Vec::new(),
        material_info: Vec::new(),
        liquid: None,
    };
    let Some(sub_chunks) = mogp.get(MOGP_HEADER_SIZE..) else {
        return Ok(g);
    };
    // A repeated sub-chunk appends: some shipped groups carry a second `MOTV`.
    for (magic, s) in chunks(sub_chunks) {
        match &magic {
            b"TVOM" => g.vertex_positions.extend(whole_records(s).map(vec3)),
            b"RNOM" => g.vertex_normals.extend(whole_records(s).map(vec3)),
            b"VTOM" => g
                .texture_coords
                .extend(whole_records(s).map(|c: &[u8; 8]| Uv {
                    u: f32_le(c, 0),
                    v: f32_le(c, 4),
                })),
            b"IVOM" => g
                .vertex_indices
                .extend(whole_records(s).map(|c: &[u8; 2]| u16_le(c, 0))),
            b"ABOM" => g
                .render_batches
                .extend(whole_records(s).map(|c: &[u8; 24]| Batch {
                    start_index: u32_le(c, 12),
                    count: u16_le(c, 16),
                    material_id: c[23],
                })),
            b"VCOM" => g
                .vertex_colors
                .extend(whole_records(s).map(|&[b, g, r, a]| Color { b, g, r, a })),
            b"YPOM" => g.material_info.extend(
                whole_records(s).map(|&[flags, material_id]| MopyEntry { flags, material_id }),
            ),
            b"QILM" => g.liquid = parse_mliq(s)?,
            _ => {}
        }
    }
    Ok(g)
}

fn vec3(c: &[u8; 12]) -> Vec3 {
    Vec3 {
        x: f32_le(c, 0),
        y: f32_le(c, 4),
        z: f32_le(c, 8),
    }
}
