use std::collections::HashMap;

use wowfile::{ByteExt, chunks};

use crate::Error;
use crate::record::{bgr_to_rgb, u32_le, whole_records};

/// The root file: textures, materials and the group count.
#[derive(Debug, Clone, PartialEq)]
pub struct WmoRoot {
    pub textures: Vec<String>,
    /// Maps a name's byte offset in the `MOTX` blob to its index in `textures`; materials name
    /// their textures by offset.
    pub texture_offset_index_map: HashMap<u32, u32>,
    pub materials: Vec<Material>,
    pub n_groups: u32,
    /// The building's own box, lower corner then upper, in its model space.
    pub bounds: [[f32; 3]; 2],
}

/// One `MOMT` material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Material {
    pub flags: u32,
    pub blend_mode: u32,
    /// Byte offset of the first texture's name in the `MOTX` blob.
    pub texture_1: u32,
    /// Self-illumination colour, meaningful only when the material's self-illumination flag is set.
    pub sidn_rgb: [u8; 3],
    pub diff_color: [u8; 3],
    /// A `TerrainType.dbc` id: the surface's footstep sound.
    pub ground_type: u32,
}

impl Material {
    /// The first texture's index in [`WmoRoot::textures`], if its offset starts a name.
    pub fn texture_1_index(&self, texture_offset_index_map: &HashMap<u32, u32>) -> Option<u32> {
        texture_offset_index_map.get(&self.texture_1).copied()
    }
}

const MOMT_SIZE: usize = 64;
const MOHD_BOX_MIN: usize = 0x24;
const MOHD_BOX_MAX: usize = 0x30;

pub(crate) fn parse_root(b: &[u8]) -> Result<WmoRoot, Error> {
    let mut root = WmoRoot {
        textures: Vec::new(),
        texture_offset_index_map: HashMap::new(),
        materials: Vec::new(),
        n_groups: 0,
        bounds: [[0.0; 3]; 2],
    };
    for (magic, p) in chunks(b) {
        match &magic {
            b"DHOM" => {
                root.n_groups = p.u32_at(4).ok_or(Error::Truncated("MOHD"))?;
                for (corner, o) in root.bounds.iter_mut().zip([MOHD_BOX_MIN, MOHD_BOX_MAX]) {
                    for (k, v) in corner.iter_mut().enumerate() {
                        *v = p.f32_at(o + 4 * k).ok_or(Error::Truncated("MOHD"))?;
                    }
                }
            }
            b"XTOM" => {
                (root.textures, root.texture_offset_index_map) = parse_motx(p);
            }
            b"TMOM" => {
                for m in whole_records::<MOMT_SIZE>(p) {
                    root.materials.push(Material {
                        flags: u32_le(m, 0),
                        blend_mode: u32_le(m, 8),
                        texture_1: u32_le(m, 12),
                        sidn_rgb: bgr_to_rgb(m, 0x10),
                        diff_color: bgr_to_rgb(m, 0x1c),
                        ground_type: u32_le(m, 0x20),
                    });
                }
            }
            _ => {}
        }
    }
    Ok(root)
}

/// `MOTX` is NUL-separated names, with padding NULs between some of them.
fn parse_motx(blob: &[u8]) -> (Vec<String>, HashMap<u32, u32>) {
    let mut textures = Vec::new();
    let mut offsets = HashMap::new();
    let mut current = String::new();
    for (i, &byte) in blob.iter().enumerate() {
        if byte == 0 {
            if !current.is_empty() {
                textures.push(std::mem::take(&mut current));
            }
        } else {
            if current.is_empty() {
                offsets.insert(i as u32, textures.len() as u32);
            }
            current.push(char::from(byte));
        }
    }
    (textures, offsets)
}
