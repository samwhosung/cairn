use wowfile::ByteExt;

use crate::{BlpTexels, Error};

pub(crate) const HEADER_SIZE: usize = 148;
pub(crate) const PALETTE_SIZE: usize = 256 * 4;
const LEVELS: usize = 16;

/// Bounds what a corrupt header can make the decoder allocate; real textures stop at 1024.
pub(crate) const MAX_DIM: u32 = 8192;

/// The header, palette and level table, read once for both decoders.
pub(crate) struct Header<'a> {
    pub compression: u8,
    pub alpha_bits: u32,
    pub alpha_type: u8,
    pub width: u32,
    pub height: u32,
    offsets: [u32; LEVELS],
    sizes: [u32; LEVELS],
    /// BGRA. Every direct BLP2 carries it; only palettized files use it.
    pub palette: &'a [u8],
    pub chain: usize,
}

impl<'a> Header<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if bytes.len() < HEADER_SIZE || &bytes[0..4] != b"BLP2" {
            return Err(Error::NotBlp2);
        }
        let byte = |o| bytes.u8_at(o).ok_or(Error::Truncated("header"));
        let word = |o| bytes.u32_at(o).ok_or(Error::Truncated("header"));
        let compression = byte(8)?;
        let alpha_bits = u32::from(byte(9)?);
        let alpha_type = byte(10)?;
        let has_mipmaps = byte(11)? != 0;
        let width = word(12)?;
        let height = word(16)?;
        let mut offsets = [0; LEVELS];
        let mut sizes = [0; LEVELS];
        for i in 0..LEVELS {
            offsets[i] = word(20 + i * 4)?;
            sizes[i] = word(84 + i * 4)?;
        }
        if width > MAX_DIM || height > MAX_DIM {
            return Err(Error::DimensionsTooLarge { width, height });
        }
        let palette = bytes
            .get(HEADER_SIZE..HEADER_SIZE + PALETTE_SIZE)
            .ok_or(Error::BadColorMap)?;
        Ok(Self {
            compression,
            alpha_bits,
            alpha_type,
            width,
            height,
            offsets,
            sizes,
            palette,
            chain: mip_chain_count(width, height, has_mipmaps),
        })
    }

    /// How many levels the decoders read: level 0 always, then up to the chain count.
    pub fn levels_to_read(&self) -> usize {
        self.chain.clamp(1, LEVELS)
    }

    /// Level `level` as the header records it, or `None` where the file's chain ends early.
    pub fn level_data<'b>(&self, bytes: &'b [u8], level: usize) -> Result<Option<&'b [u8]>, Error> {
        let off = self.offsets[level] as usize;
        let sz = self.sizes[level] as usize;
        if level > 0 && (sz == 0 || off == 0) {
            return Ok(None);
        }
        bytes
            .get(off..off + sz)
            .map(Some)
            .ok_or(Error::OutOfBounds { level })
    }

    /// A DXT level as the client uploads it: `need` bytes from the level's offset, whatever size
    /// the header records. The encoder stores a level narrower or shorter than a block as fewer
    /// blocks than its grid needs, and the client fills the rest with the levels that follow.
    /// Only a level the file ends inside comes back short.
    pub fn dxt_level_span<'b>(
        &self,
        bytes: &'b [u8],
        level: usize,
        need: usize,
    ) -> Result<Option<&'b [u8]>, Error> {
        let Some(stored) = self.level_data(bytes, level)? else {
            return Ok(None);
        };
        if stored.len() >= need {
            return Ok(Some(stored));
        }
        let off = self.offsets[level] as usize;
        let end = (off + need).min(bytes.len());
        Ok(Some(&bytes[off..end]))
    }

    /// Alpha presence is `alpha_bits`' job, so an `alpha_type` the client doesn't know (some
    /// alpha-less files carry 2) is DXT1.
    pub fn dxt_texels(&self) -> BlpTexels {
        match self.alpha_type {
            1 => BlpTexels::Bc2,
            7 => BlpTexels::Bc3,
            _ => BlpTexels::Bc1,
        }
    }
}

/// The level count the renderer uploads: one short of the full chain down to 1x1.
fn mip_chain_count(width: u32, height: u32, has_mipmaps: bool) -> usize {
    if has_mipmaps {
        width.max(height).checked_ilog2().unwrap_or(0) as usize
    } else {
        0
    }
}

pub(crate) fn level_size(width: u32, height: u32, level: usize) -> (u32, u32) {
    if level == 0 {
        (width, height)
    } else {
        ((width >> level).max(1), (height >> level).max(1))
    }
}
