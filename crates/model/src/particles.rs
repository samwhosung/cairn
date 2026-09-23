use crate::{le_u16, le_u32};

const EMITTERS: usize = 0x13c;
const EMITTER_SIZE: usize = 0x1f8;
const MAX_EMITTERS: usize = 256;

pub(crate) fn emitter_bones(bytes: &[u8]) -> Vec<(u16, u32)> {
    if bytes.len() < EMITTERS + 8 || &bytes[..4] != b"MD20" {
        return Vec::new();
    }
    let count = le_u32(bytes, EMITTERS) as usize;
    let base = le_u32(bytes, EMITTERS + 4) as usize;
    if count == 0 || count > MAX_EMITTERS || base + count * EMITTER_SIZE > bytes.len() {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let e = base + i * EMITTER_SIZE;
            (le_u16(bytes, e + 0x14), le_u32(bytes, e + 0x04))
        })
        .collect()
}
