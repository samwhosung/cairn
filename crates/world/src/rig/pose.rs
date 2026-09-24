use bevy::animation::animatable::Animatable;
use bevy::app::AnimationSystems;
use bevy::math::Affine3A;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use model::{BillboardKind, ParentArm};

use super::AnimParked;
use super::anims::ModelAnimations;
use super::bake::ModelSkeleton;
use super::source::PoseSource;
use crate::billboard::parent_arm_matrix;

/// One rig's pose, on the entity that plays its animations: each bone's local transform, and the
/// model-space frames they compose to.
#[derive(Component)]
pub struct RigPose {
    /// The rig's model-space frame; its `GlobalTransform` is the model's placement.
    pub joints_root: Entity,
    pub locals: Vec<Transform>,
    /// Each bone's animated model-space frame, the parent-arm rewrite applied.
    pub model: Vec<Affine3A>,
    pub parents: Vec<i16>,
    pub(crate) kinds: Vec<Option<BillboardKind>>,
    pub(crate) arms: Vec<Option<ParentArm>>,
    pub(crate) rest_pivots: Vec<Vec3>,
    pub(crate) has_billboard: bool,
    pub(crate) has_special: bool,
    /// The entities standing in for bones something hangs off.
    pub anchors: Vec<(u16, Entity)>,
    /// A writer touched `locals` since the palette was last written.
    pub pose_dirty: bool,
}

impl RigPose {
    /// The rig at its rest pose.
    pub fn new(joints_root: Entity, skeleton: &ModelSkeleton) -> Self {
        let locals: Vec<Transform> = skeleton
            .joints
            .iter()
            .map(|j| Transform::from_translation(j.local_translation))
            .collect();
        let mut rig = Self {
            joints_root,
            model: vec![Affine3A::IDENTITY; locals.len()],
            locals,
            parents: skeleton.joints.iter().map(|j| j.parent).collect(),
            kinds: skeleton.joints.iter().map(|j| j.billboard).collect(),
            arms: skeleton.joints.iter().map(|j| j.parent_arm).collect(),
            rest_pivots: skeleton
                .joints
                .iter()
                .map(|j| j.local_translation)
                .collect(),
            has_billboard: skeleton.joints.iter().any(|j| j.billboard.is_some()),
            has_special: skeleton
                .joints
                .iter()
                .any(|j| j.billboard.is_some() || j.parent_arm.is_some()),
            anchors: Vec::new(),
            pose_dirty: true,
        };
        rig.compose();
        rig
    }

    /// Bones are parent-sorted; one whose parent does not precede it composes from the root.
    pub(crate) fn compose(&mut self) {
        for i in 0..self.locals.len() {
            let local = self.locals[i].compute_affine();
            let parent = match usize::try_from(self.parents[i]).ok().filter(|&p| p < i) {
                Some(p) => self.model[p],
                None => Affine3A::IDENTITY,
            };
            self.model[i] = match self.arms[i] {
                Some(arm) => {
                    parent_arm_matrix(arm, parent, Affine3A::IDENTITY, self.rest_pivots[i])
                }
                None => parent,
            } * local;
        }
    }

    /// The entity standing in for `bone`, spawned under the rig's root the first time it is asked
    /// for; `None` for a bone outside the skeleton.
    pub fn anchor_for(&mut self, commands: &mut Commands<'_, '_>, bone: u16) -> Option<Entity> {
        if let Some(&(_, anchor)) = self.anchors.iter().find(|&&(b, _)| b == bone) {
            return Some(anchor);
        }
        let m = self.model.get(bone as usize)?;
        let (scale, rotation, translation) = m.to_scale_rotation_translation();
        let anchor = commands
            .spawn((
                Transform {
                    translation,
                    rotation,
                    scale,
                },
                Visibility::default(),
                ChildOf(self.joints_root),
            ))
            .id();
        self.anchors.push((bone, anchor));
        Some(anchor)
    }

    /// Where `offset` under `bone` stands in the world at the current pose.
    pub fn posed_point(
        &self,
        root_global: &GlobalTransform,
        bone: u16,
        offset: Vec3,
    ) -> Option<Vec3> {
        let m = self.model.get(bone as usize)?;
        Some(
            root_global
                .affine()
                .transform_point3(m.transform_point3(offset)),
        )
    }
}

struct PlayingClip {
    node: usize,
    clip: usize,
    mask: u64,
    weight: f32,
    seek: f32,
    cursor: usize,
}

/// Samples every unparked rig's playing animations into its bone locals, reproducing what Bevy's
/// own evaluation would write: per bone and property, the nodes with a nonzero weight whose mask
/// spares the bone fold in node order; a property no playing clip keys keeps its value.
fn evaluate_rig_poses(
    mut rigs: Query<
        '_,
        '_,
        (&AnimationPlayer, &ModelAnimations, &mut RigPose),
        Without<AnimParked>,
    >,
) {
    rigs.par_iter_mut().for_each(|(player, anims, mut rig)| {
        let src = &anims.pose;
        let mut active: Vec<PlayingClip> = player
            .playing_animations()
            .filter(|(_, anim)| anim.weight() != 0.0)
            .filter_map(|(&node, anim)| {
                let pose_node = src.node(node)?;
                src.clips.get(pose_node.clip as usize)?;
                Some(PlayingClip {
                    node: node.index(),
                    clip: pose_node.clip as usize,
                    mask: pose_node.mask,
                    weight: anim.weight(),
                    seek: anim.seek_time(),
                    cursor: 0,
                })
            })
            .collect();
        active.sort_unstable_by_key(|a| a.node);
        match active.as_mut_slice() {
            [] => {}
            [one] => {
                rig.pose_dirty = true;
                for pb in &src.clips[one.clip].bones {
                    if src.bone_masks.get(pb.bone as usize).copied().unwrap_or(0) & one.mask != 0 {
                        continue;
                    }
                    let Some(tf) = rig.locals.get_mut(pb.bone as usize) else {
                        continue;
                    };
                    if let Some(v) = pb.translation.sample(one.seek) {
                        tf.translation = v;
                    }
                    if let Some(v) = pb.rotation.sample(one.seek) {
                        tf.rotation = v;
                    }
                    if let Some(v) = pb.scale.sample(one.seek) {
                        tf.scale = v;
                    }
                }
            }
            many => {
                rig.pose_dirty = true;
                blend_rig(src, many, &mut rig);
            }
        }
    });
}

fn blend_rig(src: &PoseSource, active: &mut [PlayingClip], rig: &mut RigPose) {
    while let Some(bone) = active
        .iter()
        .filter_map(|a| src.clips[a.clip].bones.get(a.cursor).map(|b| b.bone))
        .min()
    {
        let bone_mask = src.bone_masks.get(bone as usize).copied().unwrap_or(0);
        let mut translation: Option<(Vec3, f32)> = None;
        let mut rotation: Option<(Quat, f32)> = None;
        let mut scale: Option<(Vec3, f32)> = None;
        for a in active.iter_mut() {
            let Some(pb) = src.clips[a.clip]
                .bones
                .get(a.cursor)
                .filter(|b| b.bone == bone)
            else {
                continue;
            };
            a.cursor += 1;
            if bone_mask & a.mask != 0 {
                continue;
            }
            fold(&mut translation, pb.translation.sample(a.seek), a.weight);
            fold(&mut rotation, pb.rotation.sample(a.seek), a.weight);
            fold(&mut scale, pb.scale.sample(a.seek), a.weight);
        }
        let Some(tf) = rig.locals.get_mut(bone as usize) else {
            continue;
        };
        if let Some((v, _)) = translation {
            tf.translation = v;
        }
        if let Some((v, _)) = rotation {
            tf.rotation = v;
        }
        if let Some((v, _)) = scale {
            tf.scale = v;
        }
    }
}

/// Bevy's blend register: the first contribution lands whole, each later one at its share of the
/// running weight.
#[inline]
fn fold<T: Animatable>(register: &mut Option<(T, f32)>, sample: Option<T>, weight: f32) {
    let Some(v) = sample else { return };
    *register = Some(match register.take() {
        None => (v, weight),
        Some((acc, total)) => {
            let total = total + weight;
            (T::interpolate(&acc, &v, weight / total), total)
        }
    });
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        evaluate_rig_poses
            .in_set(AnimationSystems)
            .after(bevy::animation::advance_animations)
            .before(TransformSystems::Propagate),
    );
}

#[cfg(test)]
mod tests;
