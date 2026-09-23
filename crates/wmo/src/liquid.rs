use wowfile::ByteExt;

use crate::Error;
use crate::record::{f32_le, u16_le, whole_records};

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

/// A header cut after the grid size is an error; any other malformed grid is no liquid.
pub(crate) fn parse_mliq(s: &[u8]) -> Result<Option<WmoLiquid>, Error> {
    let (Some(xverts), Some(yverts), Some(xtiles), Some(ytiles)) =
        (s.u32_at(0), s.u32_at(4), s.u32_at(8), s.u32_at(12))
    else {
        return Ok(None);
    };
    let nverts = (xverts as usize).saturating_mul(yverts as usize);
    let ntiles = (xtiles as usize).saturating_mul(ytiles as usize);
    if nverts == 0 || ntiles == 0 || nverts > MAX_CELLS || ntiles > MAX_CELLS {
        return Ok(None);
    }
    let Some(header) = s.first_chunk::<HEADER_SIZE>() else {
        return Err(Error::Truncated("MLIQ"));
    };
    let tiles_start = HEADER_SIZE + nverts * VERTEX_SIZE;
    let Some(tile_flags) = s.get(tiles_start..tiles_start + ntiles) else {
        return Ok(None);
    };
    let vertices = whole_records::<VERTEX_SIZE>(&s[HEADER_SIZE..tiles_start]);
    Ok(Some(WmoLiquid {
        xverts,
        yverts,
        xtiles,
        ytiles,
        base: [f32_le(header, 16), f32_le(header, 20), f32_le(header, 24)],
        material_id: u16_le(header, 28),
        heights: vertices.clone().map(|v| f32_le(v, 4)).collect(),
        opacity: vertices.map(|v| v[0]).collect(),
        tile_flags: tile_flags.to_vec(),
    }))
}
