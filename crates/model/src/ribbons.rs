use std::collections::HashMap;

use crate::particles::{ParticleBlend, texture_names};
use crate::value_track::{ValueTrack, seq0_band, track_keys_with};
use crate::{le_f32, le_u16, le_u32};

const RIBBONS: usize = 0x134;
const RIBBON_SIZE: usize = 0xdc;
const MAX_RIBBONS: usize = 256;
/// The render-flags table: `(flags, blend)` as two u16s an entry.
const RENDER_FLAGS: usize = 0x84;

fn track_first<T>(
    b: &[u8],
    track: usize,
    elem_size: usize,
    default: T,
    read: impl Fn(&[u8], usize) -> T,
) -> T {
    if track + 0x1c > b.len() {
        return default;
    }
    let n = le_u32(b, track + 0x14);
    let ofs = le_u32(b, track + 0x18) as usize;
    if n == 0 || ofs + elem_size > b.len() {
        return default;
    }
    read(b, ofs)
}

/// A trail of edges left behind a bone.
#[derive(Debug, Clone)]
pub struct RibbonEmitterDef {
    pub bone: u16,
    pub position: [f32; 3],
    pub texture: Option<String>,
    pub blend: ParticleBlend,
    /// The raw blend mode behind [`Self::blend`], `None` when the material does not resolve.
    pub blend_mode: Option<u16>,
    /// RGB, white when unkeyed.
    pub color: ValueTrack<[f32; 3]>,
    /// Opacity, 1 when unkeyed; may be negative.
    pub alpha: ValueTrack,
    /// Yards above and below the path. An edge keeps the heights it was committed with.
    pub height_above: ValueTrack,
    pub height_below: ValueTrack,
    pub edges_per_second: f32,
    /// Seconds an edge lives, never below a quarter second.
    pub edge_lifetime: f32,
    /// Committed edges rise by `gravity · t²` yards.
    pub gravity: f32,
    /// The texture atlas, never below `1×1`, and the cell drawn.
    pub tile_rows: u16,
    pub tile_cols: u16,
    pub tex_slot: u16,
    /// Where the trail is on and off, per animation. `None`, the trail is always on: no sequence
    /// turns it off, or its track runs on a global sequence, does not fit the file or is past the
    /// file's key budget.
    pub visible: Option<RibbonVisibility>,
}

/// A ribbon's on/off track, per animation id: its keys inside that sequence's band, in seconds
/// from the band start, the first being the value the band opens on.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonVisibility {
    by_anim: HashMap<u16, Vec<(f32, bool)>>,
}

impl RibbonVisibility {
    /// Whether the trail is on `t` seconds into animation `anim`, stepping key to key. An
    /// animation the model does not have answers as `Stand` does, and without that, on.
    pub fn at(&self, anim: u16, t: f32) -> bool {
        let Some(keys) = self.by_anim.get(&anim).or_else(|| self.by_anim.get(&0)) else {
            return true;
        };
        keys.iter()
            .take_while(|&&(kt, _)| kt <= t)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, on)| on)
    }

    /// Every animation id and its keys, in no order.
    pub fn per_anim(&self) -> impl Iterator<Item = (u16, &[(f32, bool)])> {
        self.by_anim.iter().map(|(&a, k)| (a, k.as_slice()))
    }
}

fn visibility_by_anim(bytes: &[u8], vis_track: usize) -> Option<RibbonVisibility> {
    if vis_track + 0x1c > bytes.len() {
        return None;
    }
    if le_u16(bytes, vis_track + 0x02) != 0xffff {
        return None;
    }
    let tn = le_u32(bytes, vis_track + 0x0c) as usize;
    let tofs = le_u32(bytes, vis_track + 0x10) as usize;
    let vn = le_u32(bytes, vis_track + 0x14) as usize;
    let vofs = le_u32(bytes, vis_track + 0x18) as usize;
    let n = tn.min(vn);
    if n == 0 || tofs + n * 4 > bytes.len() || vofs + n > bytes.len() {
        return None;
    }
    let keys: Vec<(u32, bool)> = (0..n)
        .map(|i| (le_u32(bytes, tofs + i * 4), bytes[vofs + i] != 0))
        .collect();
    let nseq = le_u32(bytes, 0x1c) as usize;
    let oseq = le_u32(bytes, 0x20) as usize;
    let mut by_anim: HashMap<u16, Vec<(f32, bool)>> = HashMap::new();
    let mut any_off = false;
    for i in 0..nseq {
        let s = oseq + i * 0x44;
        if s + 0x0c > bytes.len() {
            break;
        }
        let anim = le_u16(bytes, s);
        let (start, end) = (le_u32(bytes, s + 4), le_u32(bytes, s + 8));
        let opening = keys
            .iter()
            .take_while(|&&(t, _)| t <= start)
            .last()
            .or(keys.first())
            .is_some_and(|&(_, v)| v);
        let mut band: Vec<(f32, bool)> = vec![(0.0, opening)];
        for &(t, v) in keys.iter().filter(|&&(t, _)| t > start && t <= end) {
            let local = (t - start) as f32 / 1000.0;
            if band.last().is_some_and(|&(_, prev)| prev != v) {
                band.push((local, v));
            }
        }
        any_off |= band.iter().any(|&(_, v)| !v);
        by_anim.entry(anim).or_insert(band);
    }
    any_off.then_some(RibbonVisibility { by_anim })
}

/// The M2's ribbon emitters; none when the file has no table that fits.
pub fn parse_m2_ribbon_emitters(bytes: &[u8]) -> Vec<RibbonEmitterDef> {
    if bytes.len() < RIBBONS + 8 || &bytes[0..4] != b"MD20" {
        return Vec::new();
    }
    let textures = texture_names(bytes);
    let count = le_u32(bytes, RIBBONS) as usize;
    let base = le_u32(bytes, RIBBONS + 4) as usize;
    if count == 0 || count > MAX_RIBBONS || base + count * RIBBON_SIZE > bytes.len() {
        return Vec::new();
    }
    let rf_count = le_u32(bytes, RENDER_FLAGS) as usize;
    let rf_base = le_u32(bytes, RENDER_FLAGS + 4) as usize;
    let band = seq0_band(bytes);
    (0..count)
        .map(|i| {
            let e = base + i * RIBBON_SIZE;
            let first_index = |at: usize| {
                let n = le_u32(bytes, e + at);
                let ofs = le_u32(bytes, e + at + 4) as usize;
                (n > 0 && ofs + 2 <= bytes.len()).then(|| le_u16(bytes, ofs) as usize)
            };
            let texture = first_index(0x14).and_then(|ti| textures.get(ti).cloned().flatten());
            let blend_mode = first_index(0x1c)
                .filter(|&m| m < rf_count && rf_base + m * 4 + 4 <= bytes.len())
                .map(|m| le_u16(bytes, rf_base + m * 4 + 2));
            RibbonEmitterDef {
                bone: le_u16(bytes, e + 0x04),
                position: [
                    le_f32(bytes, e + 0x08),
                    le_f32(bytes, e + 0x0c),
                    le_f32(bytes, e + 0x10),
                ],
                texture,
                blend: ribbon_blend(blend_mode),
                blend_mode,
                color: track_keys_with(bytes, e + 0x24, [1.0; 3], band, 12, |b, o| {
                    [le_f32(b, o), le_f32(b, o + 4), le_f32(b, o + 8)]
                }),
                alpha: track_keys_with(bytes, e + 0x40, 1.0, band, 2, |b, o| {
                    f32::from(le_u16(b, o) as i16) / 32767.0
                }),
                height_above: track_keys_with(bytes, e + 0x5c, 0.0, band, 4, le_f32),
                height_below: track_keys_with(bytes, e + 0x78, 0.0, band, 4, le_f32),
                edges_per_second: le_f32(bytes, e + 0x94),
                edge_lifetime: le_f32(bytes, e + 0x98).max(0.25),
                gravity: le_f32(bytes, e + 0x9c),
                tile_rows: le_u16(bytes, e + 0xa0).max(1),
                tile_cols: le_u16(bytes, e + 0xa2).max(1),
                tex_slot: track_first(bytes, e + 0xa4, 2, 0, le_u16),
                visible: visibility_by_anim(bytes, e + 0xc0),
            }
        })
        .collect()
}

/// The multiplies land on `Opaque` here and on `Alpha` for particles; an unresolved material is
/// additive.
fn ribbon_blend(mode: Option<u16>) -> ParticleBlend {
    match mode {
        Some(3 | 4) | None => ParticleBlend::Add,
        Some(2) => ParticleBlend::Alpha,
        Some(1) => ParticleBlend::AlphaKey,
        Some(_) => ParticleBlend::Opaque,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_the_file_cannot_hold_yields_nothing() {
        let mut b = vec![0u8; RIBBONS + 8];
        b[0..4].copy_from_slice(b"MD20");
        b[RIBBONS..RIBBONS + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).is_empty());
        b[RIBBONS..RIBBONS + 4].copy_from_slice(&3u32.to_le_bytes());
        b[RIBBONS + 4..RIBBONS + 8].copy_from_slice(&((RIBBONS + 8) as u32).to_le_bytes());
        assert!(parse_m2_ribbon_emitters(&b).is_empty());
    }

    #[test]
    fn visibility_steps_and_falls_back_to_stand() {
        let vis = RibbonVisibility {
            by_anim: [
                (0u16, vec![(0.0, false)]),
                (153u16, vec![(0.0, false), (0.2, true), (1.4, false)]),
            ]
            .into_iter()
            .collect(),
        };
        assert!(!vis.at(153, 0.199));
        assert!(vis.at(153, 0.2), "a key takes effect at its time");
        assert!(vis.at(153, 1.399));
        assert!(!vis.at(153, 1.4));
        assert!(!vis.at(147, 0.0), "an animation it lacks answers as Stand");
        let no_stand = RibbonVisibility {
            by_anim: [(153u16, vec![(0.0, false)])].into_iter().collect(),
        };
        assert!(no_stand.at(147, 0.0));
    }
}
