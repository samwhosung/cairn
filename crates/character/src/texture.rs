use mpq::Chain;

use crate::Error;
use crate::table::read;

pub(crate) type Tile = (u32, u32, u32, u32);

/// A texture's stored mip levels decoded to RGBA8, level 0 first. Never empty: a texture without
/// mipmaps is its level 0 alone.
#[derive(Debug, Clone)]
pub struct MipChain {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

impl MipChain {
    /// The size of level `level`, at least 1 × 1.
    pub fn mip_size(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }
}

/// Reads a BLP texture off the chain (`/` or `\` separated) and decodes the mip levels it
/// stores, as many as its header counts.
pub fn read_mip_chain(chain: &Chain, path: &str) -> Result<MipChain, Error> {
    let name = path.replace('/', "\\");
    let bytes = read(chain, &name)?;
    let decoded = blp::decode(&bytes).map_err(|source| Error::Decode { path: name, source })?;
    let levels = decoded.mip_chain_count().max(1);
    Ok(MipChain {
        width: decoded.width,
        height: decoded.height,
        mips: decoded
            .mips
            .into_iter()
            .take(levels)
            .map(|m| m.rgba)
            .collect(),
    })
}

pub(crate) fn blit_over(dst: &mut MipChain, src: &MipChain, (tx, ty, tw, th): Tile) {
    let levels = dst.mips.len().min(src.mips.len());
    for i in 0..levels {
        let dw = (dst.width >> i).max(1) as usize;
        let dh = (dst.height >> i).max(1) as usize;
        let sw = (src.width >> i).max(1) as usize;
        let sh = (src.height >> i).max(1) as usize;
        let (ox, oy) = ((tx >> i) as usize, (ty >> i) as usize);
        let cw = ((tw >> i).max(1) as usize)
            .min(sw)
            .min(dw.saturating_sub(ox));
        let ch = ((th >> i).max(1) as usize)
            .min(sh)
            .min(dh.saturating_sub(oy));
        let (d, s) = (&mut dst.mips[i], &src.mips[i]);
        for row in 0..ch {
            for col in 0..cw {
                let si = (row * sw + col) * 4;
                let di = ((oy + row) * dw + (ox + col)) * 4;
                if si + 4 > s.len() || di + 4 > d.len() {
                    continue;
                }
                let a = u32::from(s[si + 3]);
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    d[di..di + 4].copy_from_slice(&s[si..si + 4]);
                    continue;
                }
                let ia = 255 - a;
                for c in 0..3 {
                    d[di + c] =
                        ((u32::from(s[si + c]) * a + u32::from(d[di + c]) * ia + 127) / 255) as u8;
                }
                d[di + 3] = (a + (u32::from(d[di + 3]) * ia + 127) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, px: Vec<u8>) -> MipChain {
        MipChain {
            width,
            height,
            mips: vec![px],
        }
    }

    #[test]
    fn blit_over_replaces_blends_and_clamps() {
        let mut dst = image(2, 2, [128, 128, 128, 255].repeat(4));

        blit_over(&mut dst, &image(1, 1, vec![255, 0, 0, 255]), (0, 0, 1, 1));
        assert_eq!(&dst.mips[0][0..4], &[255, 0, 0, 255], "opaque replaces");
        assert_eq!(&dst.mips[0][4..8], &[128, 128, 128, 255], "neighbour kept");

        blit_over(&mut dst, &image(1, 1, vec![0, 0, 255, 128]), (1, 1, 1, 1));
        assert_eq!(
            &dst.mips[0][12..16],
            &[64, 64, 192, 255],
            "half alpha blends"
        );

        let before = dst.mips[0].clone();
        blit_over(&mut dst, &image(1, 1, vec![1, 2, 3, 0]), (0, 0, 1, 1));
        assert_eq!(dst.mips[0], before, "a transparent texel leaves the base");
    }

    #[test]
    fn blit_over_halves_the_tile_per_level_and_clips() {
        let mut dst = MipChain {
            width: 4,
            height: 4,
            mips: vec![vec![0; 64], vec![0; 16], vec![0; 4]],
        };
        let src = MipChain {
            width: 4,
            height: 2,
            mips: vec![[9, 9, 9, 255].repeat(8), [7, 7, 7, 255].repeat(2)],
        };
        blit_over(&mut dst, &src, (2, 2, 4, 2));
        let lit = |level: &[u8]| -> Vec<usize> {
            (0..level.len() / 4)
                .filter(|&i| level[i * 4] != 0)
                .collect()
        };
        assert_eq!(lit(&dst.mips[0]), [10, 11, 14, 15]);
        assert_eq!(lit(&dst.mips[1]), [3]);
        assert_eq!(dst.mips[2], [0; 4], "past the source's levels");
    }
}
