/// The texture set and render path of a liquid surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    /// Water: `XTextures\river\lake_a`.
    Still,
    /// Fast water: `XTextures\river\fast_a`.
    Rapids,
    /// `XTextures\ocean\ocean_h`.
    Ocean,
    /// `XTextures\lava\lava`.
    Magma,
    /// `XTextures\slime\slime`, drawn as magma is.
    Slime,
}

impl LiquidKind {
    /// The kind a tile's type nibble (`flags & 0xf`) selects; `None` for a hole (`0xf`) or a type
    /// with no texture.
    pub fn from_nibble(nibble: u8) -> Option<LiquidKind> {
        match nibble & 0xf {
            0 | 4 => Some(LiquidKind::Still),
            1 => Some(LiquidKind::Ocean),
            2 | 6 => Some(LiquidKind::Magma),
            3 | 7 => Some(LiquidKind::Slime),
            8 => Some(LiquidKind::Rapids),
            _ => None,
        }
    }

    /// Magma and slime: drawn unlit, their texture the whole colour, and still fogged.
    pub fn is_fullbright(self) -> bool {
        matches!(self, LiquidKind::Magma | LiquidKind::Slime)
    }
}

/// A liquid surface as a regular vertex grid in model space (WoW axes, yards, Z up), with the
/// triangles that draw it. Drawn two-sided, so the winding means nothing.
#[derive(Debug, Clone)]
pub struct LiquidMesh {
    /// Vertex counts `[cols, rows]`; every per-vertex array is this grid, row-major.
    pub grid: [u32; 2],
    /// Per cell, row-major over `(cols − 1) × (rows − 1)`: whether it holds liquid.
    pub wet: Vec<bool>,
    /// Per cell: tile flag `0x80`, set on a cell a neighbouring group's liquid also covers. The
    /// client draws such a cell from one of the two groups only.
    pub shared: Vec<bool>,
    pub positions: Vec<[f32; 3]>,
    /// One texture repeat per grid cell, anchored to model space so neighbouring surfaces share
    /// one field.
    pub uvs: Vec<[f32; 2]>,
    /// Per vertex, the water's opacity coordinate in `0..=1` between the shallow and deep alphas;
    /// `0` for magma and slime, which have none.
    pub depths: Vec<f32>,
    /// Triangles into `positions`, two per wet cell.
    pub indices: Vec<u32>,
    /// The type nibble `kind` came from; it also keys the liquid's sounds.
    pub sound_nibble: u8,
    /// The liquid's material, an index into the root's `MOMT`: an interior pool's body colour is
    /// that material's diffuse colour.
    pub material_id: Option<u16>,
    pub kind: LiquidKind,
}
