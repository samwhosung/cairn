const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

pub fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub fn hash(words: &[u64]) -> u64 {
    words
        .iter()
        .fold(0x243F_6A88_85A3_08D3, |h, &w| splitmix64(h ^ w))
}

pub fn hash_ignoring_case(s: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for b in s.bytes() {
        h ^= u64::from(b.to_ascii_lowercase());
        h = h.wrapping_mul(FNV_PRIME);
    }
    splitmix64(h)
}

pub fn hash_bytes(b: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &x in b {
        h ^= u64::from(x);
        h = h.wrapping_mul(FNV_PRIME);
    }
    splitmix64(h)
}

pub fn unit_interval(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Value noise in `[-1, 1]` at `p`, in lattice units: its lattice values are hashes of the seed and
/// the lattice point, a field fixed over the whole map.
pub fn noise(seed: u64, p: [f64; 2]) -> f64 {
    let (fx, fy) = (p[0].floor(), p[1].floor());
    let (i, j) = (fx as i64, fy as i64);
    let v = |a: i64, b: i64| unit_interval(hash(&[seed, a as u64, b as u64])) * 2.0 - 1.0;
    let (sx, sy) = (
        crate::shape::smoothstep(p[0] - fx),
        crate::shape::smoothstep(p[1] - fy),
    );
    let top = v(i, j) + (v(i + 1, j) - v(i, j)) * sx;
    let bottom = v(i, j + 1) + (v(i + 1, j + 1) - v(i, j + 1)) * sx;
    top + (bottom - top) * sy
}

pub fn key(own: u64) -> u64 {
    #[cfg(test)]
    if let Some(n) = stream::next() {
        return hash(&[0x5EED, n]);
    }
    own
}

#[cfg(test)]
pub mod stream {
    use std::cell::Cell;

    thread_local!(static AT: Cell<Option<u64>> = const { Cell::new(None) });

    /// Draw every choice on this thread from one stream, as a zone must not.
    pub fn start() {
        AT.with(|a| a.set(Some(0)));
    }

    pub fn next() -> Option<u64> {
        AT.with(|a| {
            let n = a.get()?;
            a.set(Some(n + 1));
            Some(n)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_a_field() {
        let a = noise(7, [3.25, 4.5]);
        assert_eq!(a.to_bits(), noise(7, [3.25, 4.5]).to_bits());
        assert!((-1.0..=1.0).contains(&a));
        assert_ne!(a.to_bits(), noise(8, [3.25, 4.5]).to_bits());
    }
}
