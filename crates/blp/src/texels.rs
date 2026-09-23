/// The form a [`NativeBlp`](crate::NativeBlp) level's bytes are in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlpTexels {
    /// Decoded pixels: palettized and uncompressed files have no block form.
    Rgba8Unorm,
    /// DXT1, `alpha_type` 0 or any unknown value: 8 bytes per 4x4 block.
    Bc1,
    /// DXT3, `alpha_type` 1: 16 bytes per 4x4 block.
    Bc2,
    /// DXT5, `alpha_type` 7: 16 bytes per 4x4 block.
    Bc3,
}

impl BlpTexels {
    pub fn is_block_compressed(self) -> bool {
        !matches!(self, Self::Rgba8Unorm)
    }

    /// Bytes in a `width` x `height` level; block forms round up to whole blocks, so a 1x1 level
    /// costs one block.
    pub fn level_bytes(self, width: u32, height: u32) -> usize {
        match self {
            Self::Rgba8Unorm => (width as usize) * (height as usize) * 4,
            Self::Bc1 | Self::Bc2 | Self::Bc3 => {
                let blocks = width.div_ceil(4) as usize * height.div_ceil(4) as usize;
                blocks * if self == Self::Bc1 { 8 } else { 16 }
            }
        }
    }

    pub(crate) fn codec(self) -> Option<texpresso::Format> {
        match self {
            Self::Rgba8Unorm => None,
            Self::Bc1 => Some(texpresso::Format::Bc1),
            Self::Bc2 => Some(texpresso::Format::Bc2),
            Self::Bc3 => Some(texpresso::Format::Bc3),
        }
    }
}
