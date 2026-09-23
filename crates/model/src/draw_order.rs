use std::io::Cursor;

use m2::parse_m2;

use crate::{ModelBlend, RenderSubmesh, parse_m2_animations};

const MAX_RUNG: f32 = 32.0;

/// The farthest a transparent batch's bounding-box centre lies from the model's origin, in the
/// model's own yards, over its additive, `Blend`, `Mod` and `Mod2x` batches. `0.0` when there is
/// none.
pub fn m2_owner_reach(subs: &[RenderSubmesh]) -> f32 {
    subs.iter()
        .filter(|s| {
            s.additive
                || matches!(
                    s.blend,
                    ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x
                )
        })
        .filter(|s| !s.positions.is_empty())
        .map(|s| {
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &s.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            #[allow(clippy::manual_midpoint)]
            let c: [f32; 3] = std::array::from_fn(|k| 0.5 * (lo[k] + hi[k]));
            (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()
        })
        .fold(0.0f32, f32::max)
}

/// The draw-order bias that sorts an effect after every transparent batch of an owner whose
/// batches reach `reach_world` yards, and no further: the smallest whole number of yards above
/// `max(reach_world, 0)`, at most 32.
pub fn owner_last_rung(reach_world: f32) -> f32 {
    (reach_world.max(0.0).floor() + 1.0).min(MAX_RUNG)
}

/// The closed, ascending set of values [`owner_last_rung_bucket`] snaps to; the last is
/// [`owner_last_rung`]'s cap.
pub const OWNER_RUNG_BUCKETS: [f32; 3] = [4.0, 12.0, MAX_RUNG];

/// `rung` rounded up to the next of [`OWNER_RUNG_BUCKETS`], or the last of them for anything
/// larger.
pub fn owner_last_rung_bucket(rung: f32) -> f32 {
    for b in OWNER_RUNG_BUCKETS {
        if rung <= b {
            return b;
        }
    }
    OWNER_RUNG_BUCKETS[OWNER_RUNG_BUCKETS.len() - 1]
}

/// The texture file names (`""` for a texture without one) of the batches that the first
/// nonzero-length sequence with id `anim_id` shows at its start, in batch order: those with a
/// texture record whose colour alpha and transparency weight are both above zero there, each
/// step-sampled within the sequence's own key window (a missing factor counts as `1.0`). `None`
/// when the model or its first skin does not parse, or no nonzero-length sequence has that id.
pub fn m2_sequence_visible_textures(bytes: &[u8], anim_id: u16) -> Option<Vec<String>> {
    let format = parse_m2(&mut Cursor::new(bytes)).ok()?;
    let model = format.model();
    let skin = model.parse_embedded_skin(bytes, 0).ok()?;
    let seq = parse_m2_animations(bytes)
        .into_iter()
        .find(|a| a.anim_id == anim_id)?;

    let at_band_start = |track: &m2::M2ScalarTrack| -> f32 {
        let (lo, hi) = track
            .ranges
            .get(seq.seq_index)
            .copied()
            .unwrap_or((0, track.keys.len().saturating_sub(1) as u32));
        let (lo, hi) = (
            lo as usize,
            (hi as usize).min(track.keys.len().saturating_sub(1)),
        );
        let window = track.keys.get(lo..=hi).unwrap_or(&[]);
        window
            .iter()
            .take_while(|(ts, _)| *ts <= seq.start_ms)
            .last()
            .or(window.first())
            .map_or(1.0, |&(_, v)| v)
    };

    let mut shown = Vec::new();
    for b in skin.batches() {
        let alpha = model
            .color_alpha_tracks
            .get(b.color_index as usize)
            .map_or(1.0, at_band_start);
        let weight = model
            .transparency_lookup
            .get(b.weight_combo_index as usize)
            .and_then(|&t| model.transparency_tracks.get(t as usize))
            .map_or(1.0, at_band_start);
        if alpha <= 0.0 || weight <= 0.0 {
            continue;
        }
        let Some(tex) = model
            .raw_data
            .texture_lookup_table
            .get(b.texture_combo_index as usize)
            .and_then(|&t| model.textures.get(t as usize))
        else {
            continue;
        };
        shown.push(tex.filename.string.to_string_lossy().to_string());
    }
    Some(shown)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn owner_rung_buckets_cover_every_reachable_rung() {
        let max = OWNER_RUNG_BUCKETS[OWNER_RUNG_BUCKETS.len() - 1];
        assert_eq!(owner_last_rung(f32::MAX), max, "one shared ceiling");
        let mut prev = 0.0f32;
        for r in 1..=32 {
            let b = owner_last_rung_bucket(r as f32);
            assert!(b >= r as f32, "bucket({r}) sorts under the exact rung");
            assert!(b >= prev, "bucket must be monotonic");
            assert!(
                OWNER_RUNG_BUCKETS.contains(&b),
                "bucket({r}) not in the set"
            );
            prev = b;
        }
        assert!(OWNER_RUNG_BUCKETS.windows(2).all(|w| w[0] < w[1]));
    }
}
