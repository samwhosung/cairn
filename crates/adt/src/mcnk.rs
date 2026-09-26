use wowfile::ByteExt;

use crate::error::Error;
use crate::liquid::{MclqChunk, read_mclq_blocks};

const HEADER: usize = 128;
const VERTICES: usize = 145;
const SHADOW_BYTES: usize = 512;

/// A terrain vertex normal in world axes: X north, Y west, Z up.
#[derive(Debug, Clone, Copy)]
pub struct VertexNormal {
    pub x: i8,
    pub y: i8,
    pub z: i8,
}

impl VertexNormal {
    /// `[x, y, z] / 127`.
    pub fn to_normalized(&self) -> [f32; 3] {
        [
            f32::from(self.x) / 127.0,
            f32::from(self.y) / 127.0,
            f32::from(self.z) / 127.0,
        ]
    }
}

/// MCVT: up to 145 heights, rows of 9 outer then 8 inner vertices.
#[derive(Debug, Clone)]
pub struct McvtChunk {
    pub heights: Vec<f32>,
}

/// MCNR: one normal per MCVT vertex.
#[derive(Debug, Clone)]
pub struct McnrChunk {
    pub normals: Vec<VertexNormal>,
}

#[derive(Debug, Clone, Copy)]
pub struct MclyFlags {
    pub value: u32,
}

impl MclyFlags {
    /// Whether the layer's alpha map is run-length encoded.
    pub fn alpha_map_compressed(&self) -> bool {
        self.value & 0x200 != 0
    }
}

#[derive(Debug, Clone)]
pub struct MclyLayer {
    /// Index into [`crate::RootAdt::textures`].
    pub texture_id: u32,
    pub flags: MclyFlags,
    /// Where this layer's alpha map starts in [`McalChunk::data`].
    pub offset_in_mcal: u32,
    pub effect_id: u32,
}

/// MCLY: the chunk's texture layers, the first one opaque.
#[derive(Debug, Clone)]
pub struct MclyChunk {
    pub layers: Vec<MclyLayer>,
}

/// MCAL: the alpha maps of layers after the first, back to back.
#[derive(Debug, Clone)]
pub struct McalChunk {
    pub data: Vec<u8>,
}

/// MCSH: a 64×64 one-bit shadow map, zero-padded to 512 bytes.
#[derive(Debug, Clone)]
pub struct McshChunk {
    pub shadow_map: Vec<u8>,
}

impl McshChunk {
    /// Whether texel `(x, y)` is shadowed; `false` off the map.
    pub fn is_shadowed(&self, x: usize, y: usize) -> bool {
        if x >= 64 || y >= 64 {
            return false;
        }
        let byte = self.shadow_map.get(y * 8 + x / 8).copied().unwrap_or(0);
        (byte >> (x % 8)) & 1 != 0
    }
}

/// The MCNK header flag that bars movers from the chunk.
pub const MCNK_IMPASSABLE: u32 = 0x2;

/// The MCNK header flag of a chunk under ocean.
pub const MCNK_OCEAN: u32 = 0x8;

/// The MCNK header flag that marks the chunk's alpha maps as authored at 64×64: without it the
/// client copies their last row and column from their neighbours.
pub const MCNK_DO_NOT_FIX_ALPHA: u32 = 0x8000;

/// The fields of the MCNK header that are read.
#[derive(Debug, Clone)]
pub struct McnkHeader {
    pub flags: u32,
    pub index_x: u32,
    pub index_y: u32,
    /// The `AreaTable` id of the chunk.
    pub area_id: u32,
    pub holes_low_res: u16,
    /// Which layer is dominant per cell of the 8×8 grid, two bits each, row-major.
    pub pred_tex: [u8; 16],
    /// Per cell of the 8×8 grid, one bit each, row-major: no ground-effect doodads here.
    pub no_effect_doodad: [u8; 8],
    pub position: [f32; 3],
}

impl McnkHeader {
    pub fn impassable(&self) -> bool {
        self.flags & MCNK_IMPASSABLE != 0
    }
}

/// One MCNK terrain chunk. A sub-chunk whose header offset does not land on its magic, or
/// whose own size is zero where that size is used, is `None`.
#[derive(Debug, Clone)]
pub struct McnkChunk {
    pub header: McnkHeader,
    pub heights: Option<McvtChunk>,
    pub normals: Option<McnrChunk>,
    pub layers: Option<MclyChunk>,
    pub alpha: Option<McalChunk>,
    pub shadow: Option<McshChunk>,
    /// One block per liquid flag set in the header, in file order; empty on a dry chunk.
    pub liquids: Vec<MclqChunk>,
}

fn array<const N: usize>(h: &[u8], offset: usize) -> [u8; N] {
    h.bytes_at(offset, N)
        .and_then(|s| s.try_into().ok())
        .unwrap_or([0; N])
}

pub(crate) fn read_mcnk(data: &[u8]) -> Result<McnkChunk, Error> {
    let Some(h) = data.get(..HEADER) else {
        return Err(Error::Truncated("MCNK header"));
    };
    let u32_at = |offset| h.u32_at(offset).unwrap_or_default();
    let header = McnkHeader {
        flags: u32_at(0x00),
        index_x: u32_at(0x04),
        index_y: u32_at(0x08),
        area_id: u32_at(0x34),
        holes_low_res: h.u16_at(0x3C).unwrap_or_default(),
        pred_tex: array(h, 0x40),
        no_effect_doodad: array(h, 0x50),
        position: [0x68, 0x6C, 0x70].map(|o| h.f32_at(o).unwrap_or_default()),
    };
    let sub = SubChunks { data };
    let heights = sub.find(u32_at(0x14), *b"TVCM").map(|mcvt| McvtChunk {
        heights: mcvt
            .as_chunks::<4>()
            .0
            .iter()
            .take(VERTICES)
            .map(|b| f32::from_le_bytes(*b))
            .collect(),
    });
    let normals = sub.find(u32_at(0x18), *b"RNCM").map(|mcnr| McnrChunk {
        normals: mcnr
            .as_chunks::<3>()
            .0
            .iter()
            .take(VERTICES)
            .map(|&[x, y, z]| VertexNormal {
                x: x as i8,
                y: y as i8,
                z: z as i8,
            })
            .collect(),
    });
    let layers = sub.find(u32_at(0x1C), *b"YLCM").map(|mcly| MclyChunk {
        layers: mcly
            .as_chunks::<16>()
            .0
            .iter()
            .map(|l| MclyLayer {
                texture_id: l.u32_at(0).unwrap_or_default(),
                flags: MclyFlags {
                    value: l.u32_at(4).unwrap_or_default(),
                },
                offset_in_mcal: l.u32_at(8).unwrap_or_default(),
                effect_id: l.u32_at(12).unwrap_or_default(),
            })
            .collect(),
    });
    let alpha = sub
        .find_sized_by_header(u32_at(0x24), *b"LACM", u32_at(0x28))
        .map(|mcal| McalChunk {
            data: mcal.to_vec(),
        });
    let shadow = sub.find(u32_at(0x2C), *b"HSCM").map(|mcsh| {
        let mut shadow_map = vec![0u8; SHADOW_BYTES];
        let n = mcsh.len().min(SHADOW_BYTES);
        shadow_map[..n].copy_from_slice(&mcsh[..n]);
        McshChunk { shadow_map }
    });
    let liquids = sub
        .find_sized_by_header(u32_at(0x60), *b"QLCM", u32_at(0x64))
        .map(|mclq| read_mclq_blocks(mclq, header.flags))
        .unwrap_or_default();
    Ok(McnkChunk {
        header,
        heights,
        normals,
        layers,
        alpha,
        shadow,
        liquids,
    })
}

/// Sub-chunks found through the MCNK header's offsets. Files disagree on whether an offset
/// points at the sub-chunk's magic or 8 bytes past it, so both are tried.
struct SubChunks<'a> {
    data: &'a [u8],
}

struct Located {
    payload: usize,
    declared_len: usize,
}

impl<'a> SubChunks<'a> {
    fn locate(&self, offset: u32, magic: [u8; 4]) -> Option<Located> {
        if offset == 0 {
            return None;
        }
        let offset = offset as usize;
        [Some(offset), offset.checked_sub(8)]
            .into_iter()
            .flatten()
            .find(|&at| self.data.bytes_at(at, 4) == Some(&magic[..]))
            .and_then(|at| {
                Some(Located {
                    payload: at + 8,
                    declared_len: self.data.u32_at(at + 4)? as usize,
                })
            })
    }

    fn slice(&self, start: usize, len: usize) -> &'a [u8] {
        &self.data[start..start.saturating_add(len).min(self.data.len())]
    }

    /// A sub-chunk sized by its own header, `None` when that size is zero.
    fn find(&self, offset: u32, magic: [u8; 4]) -> Option<&'a [u8]> {
        let found = self.locate(offset, magic).filter(|l| l.declared_len > 0)?;
        Some(self.slice(found.payload, found.declared_len))
    }

    /// A sub-chunk whose own size is unreliable and whose length the MCNK header gives instead.
    fn find_sized_by_header(&self, offset: u32, magic: [u8; 4], len: u32) -> Option<&'a [u8]> {
        let found = self.locate(offset, magic)?;
        Some(self.slice(found.payload, len as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::chunk;

    #[test]
    fn a_corrupt_sub_chunk_offset_is_absent() {
        for offset in [3, u32::MAX] {
            let mut data = vec![0u8; HEADER];
            data[0x14..0x18].copy_from_slice(&offset.to_le_bytes());
            let mcnk = read_mcnk(&data).expect("parses");
            assert!(mcnk.heights.is_none(), "{offset}");
        }
    }

    #[test]
    fn offsets_may_point_at_the_magic_or_past_it() {
        for offset in [128u32, 136] {
            let mut data = vec![0u8; HEADER];
            data[0x2C..0x30].copy_from_slice(&offset.to_le_bytes());
            data.extend(chunk(*b"HSCM", &[0x80; 8]));
            let mcnk = read_mcnk(&data).expect("parses");
            let shadow = mcnk.shadow.expect("MCSH found");
            assert!(shadow.is_shadowed(7, 0), "{offset}");
            assert!(!shadow.is_shadowed(6, 0), "{offset}");
            assert!(!shadow.is_shadowed(7, 1), "{offset}");
        }
    }

    #[test]
    fn mcal_takes_its_length_from_the_mcnk_header() {
        let mut data = vec![0u8; HEADER];
        data[0x24..0x28].copy_from_slice(&128u32.to_le_bytes());
        data[0x28..0x2C].copy_from_slice(&6u32.to_le_bytes());
        data.extend(chunk(*b"LACM", &[]));
        data.extend([1, 2, 3, 4, 5, 6, 7]);
        let mcnk = read_mcnk(&data).expect("parses");
        assert_eq!(mcnk.alpha.expect("MCAL found").data, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn normals_are_world_axes_in_file_order() {
        let mut data = vec![0u8; HEADER];
        data[0x18..0x1C].copy_from_slice(&128u32.to_le_bytes());
        let bytes: Vec<u8> = (0..VERTICES).flat_map(|_| [127, 0x81, 64]).collect();
        data.extend(chunk(*b"RNCM", &bytes));
        let normals = read_mcnk(&data)
            .expect("parses")
            .normals
            .expect("MCNR found");
        let n = normals.normals[VERTICES - 1];
        assert_eq!((n.x, n.y, n.z), (127, -127, 64));
        assert_eq!(
            n.to_normalized().map(f32::to_bits),
            [1.0, -1.0, 64.0 / 127.0].map(f32::to_bits)
        );
    }

    #[test]
    fn splits_the_dominant_layer_and_doodad_grids() {
        let mut data = vec![0u8; HEADER];
        data[0x40..0x50].fill(0xAA);
        data[0x50..0x58].fill(0x55);
        data[0x58] = 0xFF;
        let header = read_mcnk(&data).expect("parses").header;
        assert_eq!(header.pred_tex, [0xAA; 16]);
        assert_eq!(header.no_effect_doodad, [0x55; 8]);
    }
}
