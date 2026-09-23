use crate::mcnk::McnkChunk;

const W: usize = 64;
const TEXELS: usize = W * W;

/// A chunk's alpha maps in one 64×64 RGBA image, row-major: R, G and B are the opacity of
/// texture layers 1, 2 and 3, and A is 255.
pub struct CombinedAlphaMap {
    map: Vec<u8>,
}

impl CombinedAlphaMap {
    /// `has_big_alpha` reads uncompressed maps as 8 bits a texel instead of 4. `fix_alpha` copies
    /// the last row and column from their neighbours, for maps authored at 63×63. The 1.12.1
    /// client uses `false`, `true`.
    pub fn new(chunk: &McnkChunk, has_big_alpha: bool, fix_alpha: bool) -> Self {
        let mut map = vec![0u8; TEXELS * 4];
        for px in map.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        let mut combined = Self { map };
        let (Some(mcly), Some(mcal)) = (&chunk.layers, &chunk.alpha) else {
            return combined;
        };
        for (channel, layer) in mcly.layers.iter().skip(1).enumerate() {
            let offset = layer.offset_in_mcal as usize;
            let alpha = if layer.flags.alpha_map_compressed() {
                Some(decode_rle(&mcal.data, offset))
            } else if has_big_alpha {
                decode_8bit(&mcal.data, offset)
            } else {
                decode_4bit(&mcal.data, offset)
            };
            if let Some(alpha) = alpha {
                combined.write_channel(channel, &alpha, fix_alpha);
            }
        }
        combined
    }

    /// The RGBA bytes, 64×64×4.
    pub fn as_slice(&self) -> &[u8] {
        &self.map
    }

    fn write_channel(&mut self, channel: usize, alpha: &[u8], fix_alpha: bool) {
        if channel >= 4 {
            return;
        }
        let at = |x: usize, y: usize| (y * W + x) * 4 + channel;
        for y in 0..W {
            for x in 0..W {
                let mut a = alpha[y * W + x];
                if fix_alpha {
                    if x == W - 1 {
                        a = self.map[at(x - 1, y)];
                    }
                    if y == W - 1 {
                        a = self.map[at(x, y - 1)];
                    }
                }
                self.map[at(x, y)] = a;
            }
        }
    }
}

fn decode_8bit(raw: &[u8], offset: usize) -> Option<Vec<u8>> {
    Some(raw.get(offset..)?.get(..TEXELS)?.to_vec())
}

/// Low nibble first; a nibble `n` is `n / 15` of full.
fn decode_4bit(raw: &[u8], offset: usize) -> Option<Vec<u8>> {
    let packed = raw.get(offset..)?.get(..TEXELS / 2)?;
    Some(
        packed
            .iter()
            .flat_map(|&p| [(p & 0x0F) * 17, (p >> 4) * 17])
            .collect(),
    )
}

/// Tokens of a count in the low 7 bits and a mode in the high bit: set, repeat the next byte;
/// clear, copy the next `count` bytes. A map that ends early is padded with zeros.
fn decode_rle(raw: &[u8], mut offset: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(TEXELS);
    while out.len() < TEXELS {
        let Some(&token) = raw.get(offset) else {
            break;
        };
        offset += 1;
        let count = (token & 0x7F) as usize;
        if count == 0 {
            continue;
        }
        let room = TEXELS - out.len();
        if token & 0x80 != 0 {
            let Some(&fill) = raw.get(offset) else {
                break;
            };
            offset += 1;
            out.extend(std::iter::repeat_n(fill, count.min(room)));
        } else {
            let copied = &raw[offset.min(raw.len())..];
            let n = count.min(room).min(copied.len());
            out.extend_from_slice(&copied[..n]);
            offset += n;
        }
    }
    out.resize(TEXELS, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_bit_alpha_reads_as_n_over_15() {
        let full = decode_4bit(&[0x8F; TEXELS / 2], 0).expect("long enough");
        assert_eq!(full[..2], [255, 136]);
        assert_eq!(decode_4bit(&[0; TEXELS / 2], 0).expect("long enough")[0], 0);
        assert!(decode_4bit(&[0; TEXELS / 2], 1).is_none());
    }

    #[test]
    fn rle_fills_copies_and_pads() {
        let rle = decode_rle(&[0x83, 7, 0x02, 1, 2, 0x00, 0x05, 9], 0);
        assert_eq!(rle[..7], [7, 7, 7, 1, 2, 9, 0]);
        assert_eq!(rle.len(), TEXELS);
        let long = decode_rle(&[0xFF, 1].repeat(40), 0);
        assert_eq!(long, vec![1; TEXELS]);
    }
}
