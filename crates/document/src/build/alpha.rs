//! A chunk's paint as the ADT keeps it: each layer above the base as a 64×64 map of 4-bit
//! opacities over what lies below, two to a byte, low nibble first, a nibble read as `n / 15`.
//! Unless a chunk sets `MCNK_DO_NOT_FIX_ALPHA` (0x8000), readers copy row and column 62 into 63;
//! the writer leaves it clear and copies them itself, so every reader sees the same weights.

use crate::zone::{TEXELS_ACROSS, TEXELS_IN_CHUNK};

pub const PACKED_BYTES: usize = TEXELS_IN_CHUNK / 2;

/// One layer's opacities, row-major, rows stepping south.
pub type AlphaMap = [u8; TEXELS_IN_CHUNK];

pub fn quantize(w: u8) -> u8 {
    ((u32::from(w) * 15 + 127) / 255) as u8
}

/// The nibbles readers see for `map`: quantized, row and column 62 copied into 63.
pub fn nibbles(map: &AlphaMap) -> AlphaMap {
    let mut g = [0u8; TEXELS_IN_CHUNK];
    for (k, &w) in map.iter().enumerate() {
        g[k] = quantize(w);
    }
    for r in 0..TEXELS_ACROSS {
        g[r * TEXELS_ACROSS + 63] = g[r * TEXELS_ACROSS + 62];
    }
    for c in 0..TEXELS_ACROSS {
        g[63 * TEXELS_ACROSS + c] = g[62 * TEXELS_ACROSS + c];
    }
    g
}

pub fn pack(g: &AlphaMap) -> [u8; PACKED_BYTES] {
    let mut out = [0u8; PACKED_BYTES];
    for (i, b) in out.iter_mut().enumerate() {
        *b = g[2 * i] | (g[2 * i + 1] << 4);
    }
    out
}

/// The opacity of each layer above the base: layer k covers `w_k / (w_0 + … + w_k)` of what lies
/// under it, so the blend shows each texture at its weight.
pub fn alpha_maps(w: &[&[u8]]) -> Vec<AlphaMap> {
    let mut out = Vec::new();
    for k in 1..w.len() {
        let mut m: AlphaMap = [0; TEXELS_IN_CHUNK];
        for (t, a) in m.iter_mut().enumerate() {
            let s: u32 = w[..=k].iter().map(|l| u32::from(l[t])).sum();
            let wk = u32::from(w[k][t]);
            *a = (wk * 255 + s / 2).checked_div(s).unwrap_or(0) as u8;
        }
        out.push(m);
    }
    out
}

/// Which layer's ground effect each of a chunk's 8×8 cells takes, as two bits a cell: the layer
/// with the most weight there under the layer-over-layer blend, the higher on a tie.
pub fn predominant(maps: &[AlphaMap]) -> [u8; 16] {
    let n = maps.len() + 1;
    let mut grid = [0u8; 16];
    if n < 2 {
        return grid;
    }
    for cell in 0..64 {
        let (cr, cc) = (cell / 8, cell % 8);
        let mut w = vec![0f64; n];
        for t in 0..64 {
            let k = (cr * 8 + t / 8) * TEXELS_ACROSS + cc * 8 + t % 8;
            let mut rest = 1.0;
            for l in (1..n).rev() {
                let a = f64::from(maps[l - 1][k]) / 15.0;
                w[l] += rest * a;
                rest *= 1.0 - a;
            }
            w[0] += rest;
        }
        let best = (0..n)
            .max_by(|&a, &b| w[a].total_cmp(&w[b]).then(a.cmp(&b)))
            .unwrap_or(0);
        grid[cell / 4] |= (best as u8) << (2 * (cell % 4));
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_weight_takes_the_nearest_nibble() {
        for w in 0..=255u8 {
            let err = (i32::from(w) - i32::from(quantize(w)) * 17).abs();
            assert!(err <= 8, "{w}");
            for n in 0..16u8 {
                assert!((i32::from(w) - i32::from(n) * 17).abs() >= err);
            }
        }
    }

    #[test]
    fn packing_keeps_the_nibbles_and_the_edge_rule() {
        let mut m = [0u8; TEXELS_IN_CHUNK];
        for (k, v) in m.iter_mut().enumerate() {
            *v = ((k / TEXELS_ACROSS * 4 + k % TEXELS_ACROSS) % 256) as u8;
        }
        let g = nibbles(&m);
        let p = pack(&g);
        for r in 0..TEXELS_ACROSS {
            for c in 0..TEXELS_ACROSS {
                let k = r * TEXELS_ACROSS + c;
                let packed = p[k / 2] >> (4 * (c % 2)) & 15;
                let want = quantize(m[r.min(62) * TEXELS_ACROSS + c.min(62)]);
                assert_eq!(packed, want, "{r},{c}");
            }
        }
    }
}
