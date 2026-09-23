use wowfile::capped;

use crate::{le_f32, le_u16, le_u32};

/// One bone channel on a global sequence: `(ms, value)` keys in file order, looping every
/// `period_ms`.
#[derive(Debug, Clone)]
pub struct GlobalSeqChannel<T> {
    pub period_ms: u32,
    pub keys: Vec<(u32, T)>,
}

/// A bone's global-sequence channels, each with its own period: the multi-key tracks
/// [`parse_m2_animations`](crate::parse_m2_animations) leaves out of the clips.
#[derive(Debug, Clone)]
pub struct GlobalSeqBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<[f32; 3]>>,
    pub rotation: Option<GlobalSeqChannel<[f32; 4]>>,
    pub scale: Option<GlobalSeqChannel<[f32; 3]>>,
}

fn read_global_channel<T>(
    b: &[u8],
    track: usize,
    stride: usize,
    period_of: &impl Fn(u16) -> Option<u32>,
    read_value: impl Fn(&[u8], usize) -> T,
) -> Option<GlobalSeqChannel<T>> {
    if track + 0x1c > b.len() {
        return None;
    }
    let gseq = le_u16(b, track + 0x02);
    if gseq == 0xffff {
        return None;
    }
    let period_ms = period_of(gseq)?;
    let n = le_u32(b, track + 0x0c) as usize;
    let ts_o = le_u32(b, track + 0x10) as usize;
    let val_o = le_u32(b, track + 0x18) as usize;
    if n <= 1 {
        return None;
    }
    let mut keys = Vec::with_capacity(capped(n, 4, b.len().saturating_sub(ts_o)));
    for k in 0..n {
        let (t_off, v_off) = (ts_o + k * 4, val_o + k * stride);
        if t_off + 4 > b.len() || v_off + stride > b.len() {
            break;
        }
        keys.push((le_u32(b, t_off), read_value(b, v_off)));
    }
    (keys.len() > 1).then_some(GlobalSeqChannel { period_ms, keys })
}

/// Parse every bone's global-sequence channels, leaving out bones without one. Empty when `b` is
/// under `0x40` bytes or not MD20.
pub fn parse_m2_global_sequence_bones(b: &[u8]) -> Vec<GlobalSeqBone> {
    if b.len() < 0x40 || &b[0..4] != b"MD20" {
        return Vec::new();
    }
    let (gseq_count, gseq_ofs) = (le_u32(b, 0x14) as usize, le_u32(b, 0x18) as usize);
    let period_of = |gseq: u16| -> Option<u32> {
        let i = gseq as usize;
        if i >= gseq_count {
            return None;
        }
        let o = gseq_ofs.checked_add(i * 4)?;
        if o + 4 > b.len() {
            return None;
        }
        let d = le_u32(b, o);
        (d > 0).then_some(d)
    };
    let vec3 = |b: &[u8], o: usize| [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)];
    let quat = |b: &[u8], o: usize| {
        [
            le_f32(b, o),
            le_f32(b, o + 4),
            le_f32(b, o + 8),
            le_f32(b, o + 12),
        ]
    };
    let (bone_count, bone_ofs) = (le_u32(b, 0x34) as usize, le_u32(b, 0x38) as usize);
    let mut out = Vec::new();
    for i in 0..bone_count {
        let brec = bone_ofs + i * 0x6c;
        if brec + 0x60 > b.len() {
            break;
        }
        let translation = read_global_channel(b, brec + 0x0c, 12, &period_of, vec3);
        let rotation = read_global_channel(b, brec + 0x28, 16, &period_of, quat);
        let scale = read_global_channel(b, brec + 0x44, 12, &period_of, vec3);
        if translation.is_some() || rotation.is_some() || scale.is_some() {
            out.push(GlobalSeqBone {
                bone: i as u16,
                translation,
                rotation,
                scale,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_m2_animations;

    #[test]
    fn hostile_event_and_key_counts_reserve_only_what_the_file_holds() {
        let mut b = vec![0u8; 0x11c];
        b[0..4].copy_from_slice(b"MD20");
        b[0x114..0x118].copy_from_slice(&u32::MAX.to_le_bytes());
        b[0x118..0x11c].copy_from_slice(&0x11cu32.to_le_bytes());
        let mut ev = [0u8; 44];
        ev[36..40].copy_from_slice(&u32::MAX.to_le_bytes());
        b.extend_from_slice(&ev);
        assert!(
            parse_m2_animations(&b).is_empty(),
            "no sequences ⇒ no clips, and no abort"
        );

        let mut track = [0u8; 0x1c];
        track[0x0c..0x10].copy_from_slice(&u32::MAX.to_le_bytes());
        let ch = read_global_channel(&track, 0, 4, &|_: u16| Some(1000u32), le_u32)
            .expect("more than one key on a live global sequence is a channel");
        assert_eq!(ch.keys.len(), 0x1c / 4);
    }
}
