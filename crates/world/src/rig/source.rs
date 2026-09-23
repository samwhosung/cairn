use std::cmp::Ordering;

use bevy::animation::animatable::Animatable;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::prelude::*;

/// One channel's keyframes, times ascending and deduplicated keeping the first sample at each time,
/// so [`Self::sample`] agrees key for key with the keyframe curve Bevy builds from the same keys.
#[derive(Clone, Debug)]
pub struct PoseTrack<T> {
    times: Vec<f32>,
    values: Vec<T>,
}

impl<T> Default for PoseTrack<T> {
    fn default() -> Self {
        Self {
            times: Vec::new(),
            values: Vec::new(),
        }
    }
}

impl<T: Animatable + Clone> PoseTrack<T> {
    /// No keys: absent. One key: a constant. More: sorted and deduplicated, and absent when fewer
    /// than two distinct times remain.
    pub fn new(keys: &[(f32, T)]) -> Self {
        match keys.len() {
            0 => Self::default(),
            1 => Self {
                times: vec![keys[0].0],
                values: vec![keys[0].1.clone()],
            },
            _ => {
                let mut sorted: Vec<(f32, T)> = keys.to_vec();
                sorted.sort_by(|(t0, _), (t1, _)| t0.total_cmp(t1));
                sorted.dedup_by_key(|(t, _)| *t);
                if sorted.len() < 2 {
                    return Self::default();
                }
                let (times, values) = sorted.into_iter().unzip();
                Self { times, values }
            }
        }
    }

    /// The value at `t`, clamped into the key span and interpolated between keys; `None` when the
    /// channel is absent.
    #[inline]
    pub fn sample(&self, t: f32) -> Option<T> {
        let times = &self.times;
        match times.len() {
            0 => None,
            1 => Some(self.values[0].clone()),
            _ => Some(
                match times.binary_search_by(|pt| pt.partial_cmp(&t).unwrap_or(Ordering::Equal)) {
                    Ok(i) => self.values[i].clone(),
                    Err(0) => self.values[0].clone(),
                    Err(i) if i >= times.len() => self.values[times.len() - 1].clone(),
                    Err(i) => {
                        let s = (t - times[i - 1]) / (times[i] - times[i - 1]);
                        T::interpolate(&self.values[i - 1], &self.values[i], s)
                    }
                },
            ),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// The end of the span a keyframe curve over these keys would cover: the last key time, or
    /// `None` for an absent or constant channel.
    pub(crate) fn curve_end(&self) -> Option<f32> {
        (self.times.len() >= 2).then(|| self.times[self.times.len() - 1])
    }
}

/// One bone's channels in one clip.
#[derive(Clone, Debug)]
pub struct PoseBone {
    pub bone: u16,
    pub translation: PoseTrack<Vec3>,
    pub rotation: PoseTrack<Quat>,
    pub scale: PoseTrack<Vec3>,
}

/// One clip's keyed bones, ascending by bone index.
#[derive(Clone, Default, Debug)]
pub struct PoseClip {
    pub bones: Vec<PoseBone>,
}

impl PoseClip {
    /// Inserts `bone` in bone order; a bone with no channel is dropped.
    pub fn push(&mut self, bone: PoseBone) {
        if bone.translation.is_empty() && bone.rotation.is_empty() && bone.scale.is_empty() {
            return;
        }
        let at = self.bones.partition_point(|b| b.bone <= bone.bone);
        self.bones.insert(at, bone);
    }
}

/// What an animation-graph node plays: a clip, and the mask groups it skips.
#[derive(Clone, Copy, Debug)]
pub struct PoseNode {
    pub clip: u32,
    pub mask: u64,
}

/// A model's clips as the pose evaluator samples them, indexed the way its animation graph is.
#[derive(Clone, Default, Debug)]
pub struct PoseSource {
    pub clips: Vec<PoseClip>,
    /// By graph node index; `None` for the root and unused slots.
    pub nodes: Vec<Option<PoseNode>>,
    /// Per bone, the mask groups it belongs to.
    pub bone_masks: Vec<u64>,
}

impl PoseSource {
    pub fn set_node(&mut self, node: AnimationNodeIndex, clip: u32, mask: u64) {
        let i = node.index();
        if self.nodes.len() <= i {
            self.nodes.resize(i + 1, None);
        }
        self.nodes[i] = Some(PoseNode { clip, mask });
    }

    #[inline]
    pub fn node(&self, node: AnimationNodeIndex) -> Option<PoseNode> {
        self.nodes.get(node.index()).copied().flatten()
    }
}

#[cfg(test)]
mod tests {
    use bevy::animation::animation_curves::AnimatableKeyframeCurve;
    use bevy::math::curve::Curve;

    use super::*;

    #[test]
    fn a_track_samples_as_the_keyframe_curve_does() {
        let vkeys = [
            (0.1, Vec3::new(1.0, 2.0, 3.0)),
            (0.35, Vec3::new(-2.0, 0.5, 4.0)),
            (0.4, Vec3::new(0.0, 1.0, 0.0)),
            (1.2, Vec3::new(5.0, -1.0, 2.0)),
        ];
        let track = PoseTrack::new(&vkeys);
        let curve = AnimatableKeyframeCurve::new(vkeys).expect("valid keys");
        for i in 0..=1400 {
            let t = -0.1 + i as f32 * 0.001;
            assert_eq!(track.sample(t), Some(curve.sample_clamped(t)), "t={t}");
        }
        let qkeys = [
            (0.0, Quat::from_rotation_y(0.3)),
            (0.25, Quat::from_rotation_x(1.4)),
            (0.6, Quat::from_axis_angle(Vec3::new(0.6, 0.8, 0.0), 2.9)),
        ];
        let track = PoseTrack::new(&qkeys);
        let curve = AnimatableKeyframeCurve::new(qkeys).expect("valid keys");
        for i in 0..=800 {
            let t = -0.05 + i as f32 * 0.001;
            assert_eq!(track.sample(t), Some(curve.sample_clamped(t)), "t={t}");
        }
    }

    #[test]
    fn presence_and_duplicates_follow_the_curve_constructor() {
        assert!(PoseTrack::<Vec3>::new(&[]).sample(0.5).is_none());
        let single = PoseTrack::new(&[(0.7, Vec3::X)]);
        for t in [-1.0, 0.0, 0.7, 3.0] {
            assert_eq!(single.sample(t), Some(Vec3::X));
        }
        let keys = [
            (0.0, Vec3::X),
            (0.5, Vec3::Y),
            (0.5, Vec3::Z),
            (1.0, Vec3::X),
        ];
        let dup = PoseTrack::new(&keys);
        let curve = AnimatableKeyframeCurve::new(keys).expect("valid keys");
        for i in 0..=1000 {
            let t = i as f32 * 0.001;
            assert_eq!(dup.sample(t), Some(curve.sample_clamped(t)), "t={t}");
        }
        assert!(
            PoseTrack::new(&[(0.5, Vec3::X), (0.5, Vec3::Y)])
                .sample(0.5)
                .is_none()
        );
    }

    #[test]
    fn clip_bones_stay_sorted_and_empty_bones_drop() {
        let bone = |i: u16| PoseBone {
            bone: i,
            translation: PoseTrack::new(&[(0.0, Vec3::X)]),
            rotation: PoseTrack::default(),
            scale: PoseTrack::default(),
        };
        let mut clip = PoseClip::default();
        for i in [3u16, 1, 2, 7, 0] {
            clip.push(bone(i));
        }
        clip.push(PoseBone {
            bone: 5,
            translation: PoseTrack::default(),
            rotation: PoseTrack::default(),
            scale: PoseTrack::default(),
        });
        let order: Vec<u16> = clip.bones.iter().map(|b| b.bone).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 7]);
    }
}
