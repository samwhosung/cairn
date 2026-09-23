use wowfile::ByteExt;

const BLOCK: usize = 0x324;
const VERTICES: usize = 81;
const CELLS: usize = 64;
/// The trailing flow data of a block is not read, so a shorter tail still yields a block.
const READ_SPAN: usize = 8 + VERTICES * 8 + CELLS;
const LIQUID_FLAGS: u32 = 0x3c;

/// One liquid vertex. The first four bytes mean different things by liquid type.
#[derive(Debug, Clone, Copy)]
pub struct LiquidVertex {
    pub union_data: [u8; 4],
    pub height: f32,
}

impl LiquidVertex {
    /// Water and ocean: the depth below the surface.
    pub fn depth_byte(&self) -> u8 {
        self.union_data[0]
    }

    /// Magma and slime: the `(s, t)` texture coordinates, continuous across chunk borders.
    pub fn texcoords(&self) -> [u16; 2] {
        [
            u16::from_le_bytes([self.union_data[0], self.union_data[1]]),
            u16::from_le_bytes([self.union_data[2], self.union_data[3]]),
        ]
    }
}

/// One MCLQ liquid block: a 9×9 grid of vertices and an 8×8 grid of cell flags.
#[derive(Debug, Clone)]
pub struct MclqChunk {
    pub min_height: f32,
    pub max_height: f32,
    pub vertices: Vec<LiquidVertex>,
    /// Row-major. The low nibble is the cell's liquid type, `0xf` for none.
    pub tile_flags: [u8; 64],
}

/// The blocks MCLQ packs back to back, one per liquid flag set in the MCNK header, or one when
/// none is set. Reading stops at the first block the data cannot cover.
pub(crate) fn read_mclq_blocks(data: &[u8], mcnk_flags: u32) -> Vec<MclqChunk> {
    let want = (mcnk_flags & LIQUID_FLAGS).count_ones().max(1) as usize;
    (0..want)
        .map_while(|i| data.get(i * BLOCK..).and_then(read_block))
        .collect()
}

fn read_block(b: &[u8]) -> Option<MclqChunk> {
    if b.len() < READ_SPAN {
        return None;
    }
    let vertices = (0..VERTICES)
        .map(|i| {
            let at = 8 + i * 8;
            Some(LiquidVertex {
                union_data: b.bytes_at(at, 4)?.try_into().ok()?,
                height: b.f32_at(at + 4)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(MclqChunk {
        min_height: b.f32_at(0)?,
        max_height: b.f32_at(4)?,
        vertices,
        tile_flags: b.bytes_at(8 + VERTICES * 8, CELLS)?.try_into().ok()?,
    })
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_short_block_is_no_liquid() {
        assert!(read_mclq_blocks(&[0u8; 4], 0x4).is_empty());
    }

    #[test]
    fn reads_one_block_per_liquid_flag_in_file_order() {
        let block = |height: f32, kind: u8| {
            let mut b = vec![0u8; BLOCK];
            b[0..4].copy_from_slice(&height.to_le_bytes());
            b[4..8].copy_from_slice(&height.to_le_bytes());
            for i in 0..VERTICES {
                b[8 + i * 8 + 4..8 + i * 8 + 8].copy_from_slice(&height.to_le_bytes());
            }
            b[8 + VERTICES * 8..READ_SPAN].fill(kind);
            b
        };
        let mut data = block(5.0, 4);
        data.extend(block(0.0, 1));
        let liquids = read_mclq_blocks(&data, 0x0c);
        assert_eq!(liquids.len(), 2);
        assert_eq!(liquids[0].tile_flags[0] & 0xf, 4);
        assert_eq!(liquids[0].min_height, 5.0);
        assert_eq!(liquids[1].tile_flags[0] & 0xf, 1);
        assert_eq!(liquids[1].min_height, 0.0);
        assert_eq!(read_mclq_blocks(&data, 0).len(), 1);
        assert_eq!(
            read_mclq_blocks(&data[..BLOCK + READ_SPAN - 1], 0x0c).len(),
            1
        );
    }

    #[test]
    fn texcoords_are_unsigned() {
        let v = LiquidVertex {
            union_data: [0xbd, 0x00, 0xe8, 0x00],
            height: 1.0,
        };
        assert_eq!(v.texcoords(), [189, 232]);
        assert_eq!(v.depth_byte(), 0xbd);
        let max = LiquidVertex {
            union_data: [0xff; 4],
            height: 1.0,
        };
        assert_eq!(max.texcoords(), [65535, 65535]);
    }
}
