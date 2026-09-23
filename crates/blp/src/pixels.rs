use crate::Error;

/// Palettized: one index per pixel into the BGRA palette, then `alpha_bits`-wide alpha.
pub(crate) fn decode_raw1(
    palette: &[u8],
    data: &[u8],
    w: u32,
    h: u32,
    alpha_bits: u32,
) -> Result<Vec<u8>, Error> {
    let px = (w as usize) * (h as usize);
    let indices = data.get(..px).ok_or(Error::Truncated("raw1 indices"))?;
    let alpha = &data[px..];
    let mut out = vec![0u8; px * 4];
    let (palette, _) = palette.as_chunks::<4>();
    let (pixels, _) = out.as_chunks_mut::<4>();
    for (i, (&index, rgba)) in indices.iter().zip(pixels).enumerate() {
        let [b, g, r, _] = palette[index as usize];
        *rgba = [r, g, b, alpha_at(alpha, i, alpha_bits)];
    }
    Ok(out)
}

/// Alpha missing from the file reads as 0 at 1 and 4 bits, but as opaque at 8.
fn alpha_at(alpha: &[u8], i: usize, alpha_bits: u32) -> u8 {
    match alpha_bits {
        1 => {
            let bit = (alpha.get(i / 8).copied().unwrap_or(0) >> (i % 8)) & 1;
            if bit == 1 { 255 } else { 0 }
        }
        4 => {
            let pair = alpha.get(i / 2).copied().unwrap_or(0);
            let nibble = if i.is_multiple_of(2) {
                pair & 0x0F
            } else {
                pair >> 4
            };
            (nibble << 4) | nibble
        }
        8 => alpha.get(i).copied().unwrap_or(255),
        _ => 255,
    }
}

/// Uncompressed BGRA8.
pub(crate) fn decode_raw3(data: &[u8], w: u32, h: u32) -> Result<Vec<u8>, Error> {
    let px = (w as usize) * (h as usize);
    let src = data.get(..px * 4).ok_or(Error::Truncated("raw3 pixels"))?;
    let mut out = vec![0u8; px * 4];
    for (rgba, &[b, g, r, a]) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(src.as_chunks::<4>().0)
    {
        *rgba = [r, g, b, a];
    }
    Ok(out)
}

/// `data` cut or zero-padded to exactly `need` bytes.
pub(crate) fn pad_to(data: &[u8], need: usize) -> Vec<u8> {
    let mut out = vec![0u8; need];
    let n = data.len().min(need);
    out[..n].copy_from_slice(&data[..n]);
    out
}
