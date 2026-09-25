use bevy::animation::AnimationClip;
use bevy::math::Mat3;
use bevy::prelude::*;
use model::{BillboardKind, GlobalSeqBone, M2Attachment, ModelAnimation, ParentArm, Skeleton};

use super::source::{PoseBone, PoseClip, PoseTrack};
use crate::coords::wow_to_bevy;

/// One bone of a model's rest skeleton in Bevy's axes.
#[derive(Clone, Copy, Debug)]
pub struct ModelJoint {
    /// `-1` for a root.
    pub parent: i16,
    /// The pivot relative to the parent's: pure translations, so the chain telescopes to the
    /// bone's model-space pivot at rest.
    pub local_translation: Vec3,
    pub billboard: Option<BillboardKind>,
    pub parent_arm: Option<ParentArm>,
}

#[derive(Clone, Default, Debug)]
pub struct ModelSkeleton {
    pub joints: Vec<ModelJoint>,
    pub spine_bone: Option<u16>,
    pub head_bone: Option<u16>,
}

/// Bones are parent-sorted, so a bone whose parent does not precede it hangs from the root.
pub(crate) fn preceding_parent(parent: i16, bone: usize) -> Option<usize> {
    usize::try_from(parent).ok().filter(|&p| p < bone)
}

/// The key-bone ids of the lower spine and the head.
const KEY_BONE_SPINE: i16 = 4;
const KEY_BONE_HEAD: i16 = 6;

/// The skeleton, and each bone's inverse bind pose: a translation by minus its pivot, since a
/// vanilla bone at rest is the identity about its pivot.
pub fn build_skeleton(skel: &Skeleton) -> (ModelSkeleton, Vec<Mat4>) {
    let pivots = skeleton_pivots(skel);
    let joints = skel
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let parent_pivot = usize::try_from(b.parent)
                .ok()
                .and_then(|p| pivots.get(p).copied())
                .unwrap_or(Vec3::ZERO);
            ModelJoint {
                parent: b.parent,
                local_translation: pivots[i] - parent_pivot,
                billboard: b.billboard,
                parent_arm: b.parent_arm,
            }
        })
        .collect();
    let inverse_bindposes = pivots.iter().map(|p| Mat4::from_translation(-*p)).collect();
    let key_bone = |k: i16| {
        skel.bones
            .iter()
            .position(|b| b.key_bone == k)
            .map(|i| i as u16)
    };
    (
        ModelSkeleton {
            joints,
            spine_bone: key_bone(KEY_BONE_SPINE),
            head_bone: key_bone(KEY_BONE_HEAD),
        },
        inverse_bindposes,
    )
}

pub fn skeleton_pivots(skel: &Skeleton) -> Vec<Vec3> {
    skel.bones.iter().map(|b| wow_to_bevy(b.pivot)).collect()
}

/// An attachment point: the bone it rides and its offset from that bone's pivot, Bevy axes.
#[derive(Clone, Copy, Debug)]
pub struct ModelAttachment {
    pub id: u16,
    pub bone: u16,
    pub offset: Vec3,
}

pub fn build_attachments(attachments: &[M2Attachment], pivots: &[Vec3]) -> Vec<ModelAttachment> {
    attachments
        .iter()
        .filter_map(|a| {
            let pivot = pivots.get(a.bone as usize)?;
            Some(ModelAttachment {
                id: a.id,
                bone: a.bone,
                offset: wow_to_bevy(a.position) - *pivot,
            })
        })
        .collect()
}

fn wow_to_bevy_quat() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        wow_to_bevy([1.0, 0.0, 0.0]),
        wow_to_bevy([0.0, 1.0, 0.0]),
        wow_to_bevy([0.0, 0.0, 1.0]),
    ))
}

/// Translation keys are offsets from the bone's rest pivot, so they land on its local translation.
/// The clip's length is what Bevy's would be with a curve per channel: the sequence's length for
/// a single-key channel, the last key's time for a multi-key one, the sequence's length when no
/// channel is keyed at all.
pub fn build_animation_clip(
    anim: &ModelAnimation,
    skeleton: &ModelSkeleton,
) -> (AnimationClip, PoseClip, bool) {
    let r = wow_to_bevy_quat();
    let mut pose = PoseClip::default();
    let mut duration: Option<f32> = None;
    let mut keep = |keys: usize, end: Option<f32>| {
        let end = match keys {
            0 => None,
            1 => Some(anim.duration.max(1e-3)),
            _ => end,
        };
        if let Some(end) = end.filter(|e| e.is_finite()) {
            duration = Some(duration.unwrap_or(0.0).max(end));
        }
    };
    for bk in &anim.bones {
        let rest = skeleton
            .joints
            .get(bk.bone as usize)
            .map_or(Vec3::ZERO, |j| j.local_translation);
        let trans: Vec<(f32, Vec3)> = bk
            .translation
            .iter()
            .map(|(t, v)| (*t, rest + wow_to_bevy(*v)))
            .collect();
        let rot: Vec<(f32, Quat)> = bk
            .rotation
            .iter()
            .map(|(t, q)| {
                (
                    *t,
                    r * Quat::from_xyzw(q[0], q[1], q[2], q[3]) * r.inverse(),
                )
            })
            .collect();
        let scale: Vec<(f32, Vec3)> = bk
            .scale
            .iter()
            .map(|(t, s)| (*t, Vec3::new(s[1], s[2], s[0])))
            .collect();
        let bone = PoseBone {
            bone: bk.bone,
            translation: PoseTrack::new(&trans),
            rotation: PoseTrack::new(&rot),
            scale: PoseTrack::new(&scale),
        };
        keep(trans.len(), bone.translation.curve_end());
        keep(rot.len(), bone.rotation.curve_end());
        keep(scale.len(), bone.scale.curve_end());
        pose.push(bone);
    }
    let mut clip = AnimationClip::default();
    let poses_bones = duration.is_some();
    clip.set_duration(duration.unwrap_or(anim.duration.max(1e-3)));
    (clip, pose, poses_bones)
}

/// A free-running bone channel on a global sequence: keys in seconds, values in the joint's
/// local frame.
#[derive(Clone, Debug)]
pub struct GlobalSeqChannel<T> {
    pub period: f32,
    pub keys: Vec<(f32, T)>,
}

impl<T: Copy> GlobalSeqChannel<T> {
    fn bracket(&self, t: f32) -> (T, T, f32) {
        let period = self.period.max(1e-3);
        let t = t.rem_euclid(period);
        let keys = &self.keys;
        if t <= keys[0].0 {
            return (keys[0].1, keys[0].1, 0.0);
        }
        for w in keys.windows(2) {
            if t <= w[1].0 {
                let span = (w[1].0 - w[0].0).max(1e-6);
                return (w[0].1, w[1].1, (t - w[0].0) / span);
            }
        }
        let last = keys[keys.len() - 1].1;
        (last, last, 0.0)
    }
}

impl GlobalSeqChannel<Vec3> {
    pub fn sample(&self, t: f32) -> Vec3 {
        let (a, b, f) = self.bracket(t);
        a.lerp(b, f)
    }
}

impl GlobalSeqChannel<Quat> {
    pub fn sample(&self, t: f32) -> Quat {
        let (a, b, f) = self.bracket(t);
        a.slerp(b, f)
    }
}

#[derive(Clone, Debug)]
pub struct GlobalBone {
    pub bone: u16,
    pub translation: Option<GlobalSeqChannel<Vec3>>,
    pub rotation: Option<GlobalSeqChannel<Quat>>,
    pub scale: Option<GlobalSeqChannel<Vec3>>,
}

pub fn build_global_bones(gseq: &[GlobalSeqBone], skeleton: &ModelSkeleton) -> Vec<GlobalBone> {
    let r = wow_to_bevy_quat();
    let ms = |t: u32| t as f32 / 1000.0;
    gseq.iter()
        .map(|g| {
            let rest = skeleton
                .joints
                .get(g.bone as usize)
                .map_or(Vec3::ZERO, |j| j.local_translation);
            GlobalBone {
                bone: g.bone,
                translation: g.translation.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, v)| (ms(*t), rest + wow_to_bevy(*v)))
                        .collect(),
                }),
                rotation: g.rotation.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, q)| {
                            (
                                ms(*t),
                                r * Quat::from_xyzw(q[0], q[1], q[2], q[3]) * r.inverse(),
                            )
                        })
                        .collect(),
                }),
                scale: g.scale.as_ref().map(|c| GlobalSeqChannel {
                    period: ms(c.period_ms),
                    keys: c
                        .keys
                        .iter()
                        .map(|(t, s)| (ms(*t), Vec3::new(s[1], s[2], s[0])))
                        .collect(),
                }),
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
    use bevy::animation::{AnimationTargetId, animated_field};
    use model::BoneKeys;

    use super::*;

    fn sequence(duration: f32, bones: Vec<BoneKeys>) -> ModelAnimation {
        ModelAnimation {
            anim_id: 0,
            seq_index: 0,
            start_ms: 0,
            end_ms: (duration * 1000.0) as u32,
            duration,
            looping: true,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_center: [0.0; 3],
            bounds_radius: 0.0,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            frequency: 0,
            min_replay: 0,
            max_replay: 0,
            bones,
            events: Vec::new(),
        }
    }

    #[test]
    fn a_boneless_sequence_still_carries_its_length() {
        let (clip, _, poses) =
            build_animation_clip(&sequence(1.333, Vec::new()), &ModelSkeleton::default());
        assert!(!poses);
        assert!((clip.duration() - 1.333).abs() < 1e-4);
    }

    #[test]
    fn the_clip_is_as_long_as_the_curves_bevy_would_build() {
        let bones = vec![
            BoneKeys {
                bone: 0,
                translation: vec![(0.1, [0.0; 3]), (0.7, [0.0, 0.0, 1.0])],
                rotation: Vec::new(),
                scale: Vec::new(),
            },
            BoneKeys {
                bone: 1,
                translation: Vec::new(),
                rotation: vec![(0.2, [0.0, 0.0, 0.0, 1.0]), (0.4, [0.0, 0.0, 0.0, 1.0])],
                scale: vec![(0.5, [1.0; 3]), (0.5, [2.0; 3])],
            },
        ];
        let seq = sequence(2.0, bones);
        let (clip, _, poses) = build_animation_clip(&seq, &ModelSkeleton::default());
        let mut twin = AnimationClip::default();
        let target = AnimationTargetId::from_name(&Name::new("b"));
        twin.add_curve_to_target(
            target,
            AnimatableCurve::new(
                animated_field!(Transform::translation),
                AnimatableKeyframeCurve::new([(0.1, Vec3::ZERO), (0.7, Vec3::Y)]).expect("keys"),
            ),
        );
        twin.add_curve_to_target(
            target,
            AnimatableCurve::new(
                animated_field!(Transform::rotation),
                AnimatableKeyframeCurve::new([(0.2, Quat::IDENTITY), (0.4, Quat::IDENTITY)])
                    .expect("keys"),
            ),
        );
        assert!(poses);
        assert_eq!(clip.duration(), twin.duration());
        assert_eq!(clip.duration(), 0.7);

        let one_key = vec![BoneKeys {
            bone: 0,
            translation: vec![(0.1, [0.0; 3])],
            rotation: Vec::new(),
            scale: Vec::new(),
        }];
        let (clip, _, _) = build_animation_clip(&sequence(2.0, one_key), &ModelSkeleton::default());
        assert_eq!(clip.duration(), 2.0);
    }

    #[test]
    fn the_rotation_conjugate_matches_the_coordinate_map() {
        let r = wow_to_bevy_quat();
        for axis in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            assert!((r * Vec3::from_array(axis)).abs_diff_eq(wow_to_bevy(axis), 1e-5));
        }
        let theta = std::f32::consts::FRAC_PI_3;
        let (s, c) = (theta / 2.0).sin_cos();
        let q = r * Quat::from_xyzw(0.0, 0.0, s, c) * r.inverse();
        assert!(q.dot(Quat::from_axis_angle(Vec3::Y, theta)).abs() > 0.9999);
    }

    #[test]
    fn a_global_channel_samples_and_wraps() {
        let ch = GlobalSeqChannel {
            period: 6.633,
            keys: vec![
                (0.0, Vec3::ZERO),
                (0.033, Vec3::ONE),
                (0.100, Vec3::ONE),
                (0.133, Vec3::ZERO),
            ],
        };
        assert!(ch.sample(0.0).abs_diff_eq(Vec3::ZERO, 1e-4));
        assert!(ch.sample(0.06).abs_diff_eq(Vec3::ONE, 1e-4));
        assert!(ch.sample(3.0).abs_diff_eq(Vec3::ZERO, 1e-4));
        assert!(ch.sample(0.0165).abs_diff_eq(Vec3::splat(0.5), 1e-2));
        assert!(ch.sample(6.633 + 0.06).abs_diff_eq(Vec3::ONE, 1e-4));
    }
}
