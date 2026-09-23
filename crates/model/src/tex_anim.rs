use m2::M2Model;

use crate::key_anim::{KeyAnim, SeqLoops, SeqSlot, bake_track};

/// A baked texture translation loop: the raw `(x, y)` UV offset.
pub type UvAnim = KeyAnim<[f32; 2]>;

impl KeyAnim<[f32; 2]> {
    /// The offset `elapsed` seconds in, on the loop's clock; `[0, 0]` without keys.
    pub fn sample(&self, elapsed: f32) -> [f32; 2] {
        self.sample_or(elapsed, [0.0, 0.0])
    }
}

/// A baked texture rotation loop of raw quaternion keys. The client lerps them per component and
/// uses the result unnormalised.
pub type UvRotAnim = KeyAnim<[f32; 4]>;

impl KeyAnim<[f32; 4]> {
    /// The quaternion `elapsed` seconds in, on the loop's clock; the identity without keys.
    pub fn sample(&self, elapsed: f32) -> [f32; 4] {
        self.sample_or(elapsed, [0.0, 0.0, 0.0, 1.0])
    }
}

fn is_zero(v: [f32; 2]) -> bool {
    v[0].abs() < 1e-6 && v[1].abs() < 1e-6
}

pub(crate) fn bake_uv_anim(
    model: &M2Model,
    combo_index: u16,
    seq0: Option<SeqSlot>,
) -> Option<UvAnim> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    bake_track(
        &t.translation,
        &model.global_sequences,
        seq0,
        |v| [v[0], v[1]],
        is_zero,
        is_zero,
    )
}

pub(crate) fn bake_uv_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 2]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.translation,
                    &model.global_sequences,
                    Some(slot),
                    |v| [v[0], v[1]],
                    is_zero,
                    is_zero,
                )
            })
            .collect(),
    )
}

fn is_quat_identity(q: [f32; 4]) -> bool {
    q[0].abs() < 1e-6 && q[1].abs() < 1e-6 && q[2].abs() < 1e-6 && (q[3] - 1.0).abs() < 1e-6
}

fn is_scale_identity(v: [f32; 2]) -> bool {
    (v[0] - 1.0).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6
}

pub(crate) fn bake_uv_rot_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 4]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.rotation,
                    &model.global_sequences,
                    Some(slot),
                    |q| q,
                    is_quat_identity,
                    is_quat_identity,
                )
            })
            .collect(),
    )
}

pub(crate) fn bake_uv_scale_seqs(
    model: &M2Model,
    combo_index: u16,
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 2]>> {
    let ti = *model.texture_transform_lookup.get(combo_index as usize)?;
    let t = model.texture_transforms.get(ti as usize)?;
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| {
                bake_track(
                    &t.scaling,
                    &model.global_sequences,
                    Some(slot),
                    |v| [v[0], v[1]],
                    is_scale_identity,
                    is_scale_identity,
                )
            })
            .collect(),
    )
}

/// `uv` under a texture transform: `R_q((uv + t − p) ⊙ s) + p` about the pivot `p = (½, ½)`,
/// the client's order of translation, then scale, then rotation. Only the quaternion's z
/// rotation is read (see [`rotation_2x2`]); a positive `z·w` turns counter-clockwise with `u`
/// right and `v` up.
pub fn uv_transform(uv: [f32; 2], t: [f32; 2], q: [f32; 4], s: [f32; 2]) -> [f32; 2] {
    let (c, sn) = rotation_2x2(q);
    let dx = (uv[0] + t[0] - 0.5) * s[0];
    let dy = (uv[1] + t[1] - 0.5) * s[1];
    [0.5 + dx * c - dy * sn, 0.5 + dx * sn + dy * c]
}

/// The `(cos, sin)` of a quaternion's z rotation as the client's matrix holds them, without
/// normalising: `(1 − 2z², 2zw)`.
pub fn rotation_2x2(q: [f32; 4]) -> (f32, f32) {
    let (z, w) = (q[2], q[3]);
    (1.0 - 2.0 * z * z, 2.0 * z * w)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use m2::M2Vec3Track;

    #[test]
    fn the_uv_law_turns_counter_clockwise_for_a_positive_z() {
        let r = std::f32::consts::FRAC_1_SQRT_2;
        let got = uv_transform([1.0, 0.5], [0.0, 0.0], [0.0, 0.0, r, r], [1.0, 1.0]);
        assert!(
            (got[0] - 0.5).abs() < 1e-6 && (got[1] - 1.0).abs() < 1e-6,
            "{got:?}"
        );
        let cw = uv_transform([1.0, 0.5], [0.0, 0.0], [0.0, 0.0, -r, r], [1.0, 1.0]);
        assert!(
            (cw[0] - 0.5).abs() < 1e-6 && (cw[1] - 0.0).abs() < 1e-6,
            "{cw:?}"
        );
        let id = uv_transform([0.2, 0.7], [0.0, 0.0], [0.0, 0.0, 0.0, 1.0], [1.0, 1.0]);
        assert!((id[0] - 0.2).abs() < 1e-6 && (id[1] - 0.7).abs() < 1e-6);
        let tr = uv_transform([0.5, 0.5], [0.5, 0.0], [0.0, 0.0, r, r], [1.0, 1.0]);
        assert!(
            (tr[0] - 0.5).abs() < 1e-6 && (tr[1] - 1.0).abs() < 1e-6,
            "the translation comes before the rotation: {tr:?}"
        );
        let (c, sn) = rotation_2x2([
            0.0,
            0.0,
            f32::midpoint(0.0, (22.5f32).to_radians().sin()),
            f32::midpoint(1.0, (22.5f32).to_radians().cos()),
        ]);
        assert!(
            (c * c + sn * sn).sqrt() < 1.0,
            "a lerped quaternion is short, so it shrinks as it turns"
        );
    }

    fn track(gseq: u16, interp: u16, keys: &[(u32, [f32; 3])]) -> M2Vec3Track {
        M2Vec3Track {
            interp,
            gseq,
            ranges: Vec::new(),
            keys: keys.to_vec(),
        }
    }

    fn bake(t: &M2Vec3Track, gseq: &[u32], seq0: Option<(u32, u32)>) -> Option<UvAnim> {
        let slot = seq0.map(|band_ms| SeqSlot {
            file_index: 0,
            band_ms,
            looping: true,
        });
        bake_track(t, gseq, slot, |v| [v[0], v[1]], is_zero, is_zero)
    }

    #[test]
    fn bake_drops_the_identity_and_keeps_a_static_shift() {
        assert_eq!(bake(&track(0xffff, 1, &[]), &[], None), None);
        assert_eq!(
            bake(
                &track(0xffff, 1, &[(0, [0.0; 3]), (500, [0.0; 3])]),
                &[],
                None
            ),
            None
        );
        let shift = bake(&track(0xffff, 1, &[(0, [0.25, 0.5, 9.0])]), &[], None).expect("shifts");
        assert_eq!(shift.period, 0.0);
        assert_eq!(shift.sample(77.0), [0.25, 0.5]);
    }

    #[test]
    fn gseq_scroll_wraps_and_lerps() {
        let a = bake(
            &track(3, 1, &[(0, [0.0; 3]), (1333, [0.0, -1.0, 0.0])]),
            &[1, 1, 1, 1333],
            None,
        )
        .expect("scrolls");
        assert!((a.period - 1.333).abs() < 1e-6);
        let mid = a.sample(1.333 / 2.0);
        assert!((mid[1] + 0.5).abs() < 1e-3, "half-loop offset ≈ -0.5 V");
        assert!((a.sample(1.333 + 0.1)[1] - a.sample(0.1)[1]).abs() < 1e-4);
    }
}
