use crate::record::{f32_le, u16_le, u32_le, whole_records};

/// A group's `MLIQ` liquid surface: a height grid of `xverts × yverts` over `xtiles × ytiles`
/// cells, in the object's model space.
#[derive(Debug, Clone, PartialEq)]
pub struct WmoLiquid {
    pub xverts: u32,
    pub yverts: u32,
    pub xtiles: u32,
    pub ytiles: u32,
    /// The grid's `(0, 0)` corner.
    pub base: [f32; 3],
    pub material_id: u16,
    /// Per vertex, row-major.
    pub heights: Vec<f32>,
    /// Per vertex, row-major: the first byte of each vertex record. For water it is an opacity
    /// index; for magma and slime it is half of a texture coordinate and means nothing alone.
    pub opacity: Vec<u8>,
    /// Per tile, row-major. The low nibble is the liquid type, `0xf` for no liquid.
    pub tile_flags: Vec<u8>,
}

const HEADER_SIZE: usize = 30;
const VERTEX_SIZE: usize = 8;
const MAX_CELLS: usize = 1 << 20;

/// A grid the payload cannot hold, header, vertices or tile flags, is no liquid.
pub(crate) fn parse_mliq(s: &[u8]) -> Option<WmoLiquid> {
    let header = s.first_chunk::<HEADER_SIZE>()?;
    let [xverts, yverts, xtiles, ytiles] = [0, 4, 8, 12].map(|at| u32_le(header, at));
    let nverts = (xverts as usize).saturating_mul(yverts as usize);
    let ntiles = (xtiles as usize).saturating_mul(ytiles as usize);
    if nverts == 0 || ntiles == 0 || nverts > MAX_CELLS || ntiles > MAX_CELLS {
        return None;
    }
    let tiles_start = HEADER_SIZE + nverts * VERTEX_SIZE;
    let tile_flags = s.get(tiles_start..tiles_start + ntiles)?;
    let vertices = whole_records::<VERTEX_SIZE>(&s[HEADER_SIZE..tiles_start]);
    Some(WmoLiquid {
        xverts,
        yverts,
        xtiles,
        ytiles,
        base: [f32_le(header, 16), f32_le(header, 20), f32_le(header, 24)],
        material_id: u16_le(header, 28),
        heights: vertices.clone().map(|v| f32_le(v, 4)).collect(),
        opacity: vertices.map(|v| v[0]).collect(),
        tile_flags: tile_flags.to_vec(),
    })
}
