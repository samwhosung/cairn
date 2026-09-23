//! Decodes World of Warcraft 1.12.1 BLP2 textures: to RGBA8 pixels, or to the DXT blocks as stored.
//!
//! The client ships three kinds of direct BLP2: palettized (a BGRA palette and one index per
//! pixel, with 0-, 1-, 4- or 8-bit alpha after the indices), uncompressed BGRA8, and DXT1/3/5.
//! [`decode`] turns every kind into RGBA8; [`decode_native`] keeps DXT levels as blocks, which is
//! what the client uploads, and decodes only the kinds that have no block form.

mod error;
mod header;
mod pixels;
#[cfg(test)]
mod tests;
mod texels;

pub use error::Error;
pub use texels::BlpTexels;

use header::{Header, level_size};
use pixels::{decode_raw1, decode_raw3, pad_to};

/// A texture decoded to RGBA8, level 0 first.
pub struct DecodedBlp {
    pub width: u32,
    pub height: u32,
    /// Never empty; holds up to [`mip_chain_count`](Self::mip_chain_count) levels, or level 0 alone
    /// when that is 0, and fewer when the file's chain ends early.
    pub mips: Vec<MipLevel>,
    mip_chain_count: usize,
}

pub struct MipLevel {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl DecodedBlp {
    /// `floor(log2(max(width, height)))` when the file has mipmaps, else 0.
    pub fn mip_chain_count(&self) -> usize {
        self.mip_chain_count
    }
}

/// A texture with its levels in the form [`texels`](Self::texels) names, level 0 first.
pub struct NativeBlp {
    pub width: u32,
    pub height: u32,
    pub texels: BlpTexels,
    /// Never empty, and as many levels as [`decode`] returns.
    pub mips: Vec<NativeMip>,
    mip_chain_count: usize,
}

pub struct NativeMip {
    pub width: u32,
    pub height: u32,
    /// Exactly [`BlpTexels::level_bytes`] long. A DXT level the file ends inside is zero-padded,
    /// and a zero block decodes to transparent black.
    pub bytes: Vec<u8>,
}

impl NativeBlp {
    /// See [`DecodedBlp::mip_chain_count`].
    pub fn mip_chain_count(&self) -> usize {
        self.mip_chain_count
    }
}

/// Decodes a BLP2 file to RGBA8.
pub fn decode(bytes: &[u8]) -> Result<DecodedBlp, Error> {
    let h = Header::parse(bytes)?;
    let mut mips = Vec::with_capacity(h.levels_to_read());
    for level in 0..h.levels_to_read() {
        let (lw, lh) = level_size(h.width, h.height, level);
        let data = if h.compression == 2 {
            h.dxt_level_span(bytes, level, h.dxt_texels().level_bytes(lw, lh))?
        } else {
            h.level_data(bytes, level)?
        };
        let Some(data) = data else {
            break;
        };
        let rgba = match h.compression {
            1 => decode_raw1(h.palette, data, lw, lh, h.alpha_bits)?,
            2 => decode_level(h.dxt_texels(), lw, lh, data),
            3 => decode_raw3(data, lw, lh)?,
            other => return Err(Error::UnknownCompression(other)),
        };
        mips.push(MipLevel {
            width: lw,
            height: lh,
            rgba,
        });
    }
    Ok(DecodedBlp {
        width: h.width,
        height: h.height,
        mips,
        mip_chain_count: h.chain,
    })
}

/// Decodes a BLP2 file keeping DXT levels as their blocks; the other kinds decode as in [`decode`].
pub fn decode_native(bytes: &[u8]) -> Result<NativeBlp, Error> {
    let h = Header::parse(bytes)?;
    let texels = if h.compression == 2 {
        h.dxt_texels()
    } else {
        BlpTexels::Rgba8Unorm
    };
    let mut mips = Vec::with_capacity(h.levels_to_read());
    for level in 0..h.levels_to_read() {
        let (lw, lh) = level_size(h.width, h.height, level);
        let need = texels.level_bytes(lw, lh);
        let data = if h.compression == 2 {
            h.dxt_level_span(bytes, level, need)?
        } else {
            h.level_data(bytes, level)?
        };
        let Some(data) = data else {
            break;
        };
        let bytes = match h.compression {
            1 => decode_raw1(h.palette, data, lw, lh, h.alpha_bits)?,
            2 => pad_to(data, need),
            3 => decode_raw3(data, lw, lh)?,
            other => return Err(Error::UnknownCompression(other)),
        };
        mips.push(NativeMip {
            width: lw,
            height: lh,
            bytes,
        });
    }
    Ok(NativeBlp {
        width: h.width,
        height: h.height,
        texels,
        mips,
        mip_chain_count: h.chain,
    })
}

/// Decodes one level of `texels` blocks to RGBA8, zero-padding `bytes` short of a whole level.
/// [`BlpTexels::Rgba8Unorm`] returns `bytes` as they are.
pub fn decode_level(texels: BlpTexels, width: u32, height: u32, bytes: &[u8]) -> Vec<u8> {
    let Some(codec) = texels.codec() else {
        return bytes.to_vec();
    };
    let (w, h) = (width as usize, height as usize);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let blocks = pad_to(bytes, codec.compressed_size(w, h));
    let mut out = vec![0u8; w * h * 4];
    codec.decompress(&blocks, w, h, &mut out);
    out
}
