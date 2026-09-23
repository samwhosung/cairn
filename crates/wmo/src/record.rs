//! Little-endian reads inside fixed-size records, where every offset is in bounds by the record's size.

pub fn u16_le<const N: usize>(r: &[u8; N], o: usize) -> u16 {
    u16::from_le_bytes([r[o], r[o + 1]])
}

pub fn u32_le<const N: usize>(r: &[u8; N], o: usize) -> u32 {
    u32::from_le_bytes([r[o], r[o + 1], r[o + 2], r[o + 3]])
}

pub fn f32_le<const N: usize>(r: &[u8; N], o: usize) -> f32 {
    f32::from_bits(u32_le(r, o))
}

pub fn bgr_to_rgb<const N: usize>(r: &[u8; N], o: usize) -> [u8; 3] {
    [r[o + 2], r[o + 1], r[o]]
}

pub fn whole_records<const N: usize>(s: &[u8]) -> std::slice::Iter<'_, [u8; N]> {
    s.as_chunks::<N>().0.iter()
}
