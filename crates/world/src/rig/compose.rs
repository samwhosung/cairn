use bevy::math::Vec3A;
use bevy::prelude::*;

use super::AnimParked;
use super::palette::{RigPalettes, RigSkin};
use super::pose::RigPose;
use crate::billboard::{billboard_basis, parent_arm_matrix};
use crate::view::WorldCamera;

/// The writers of bone locals that run after the animations are sampled; the model-space compose
/// runs after all of them.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PosePost;

/// Where every rig's palette rows are written, after transform propagation.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct RigFinalize;

fn compose_rig_models(
    mut rigs: Query<'_, '_, &mut RigPose, Without<AnimParked>>,
    mut anchors: Query<'_, '_, &mut Transform>,
) {
    for rig in &mut rigs {
        if !rig.pose_dirty {
            continue;
        }
        let rig = rig.into_inner();
        rig.compose();
        for &(bone, anchor) in &rig.anchors {
            let Some(m) = rig.model.get(bone as usize) else {
                continue;
            };
            let Ok(mut t) = anchors.get_mut(anchor) else {
                continue;
            };
            let (scale, rotation, translation) = m.to_scale_rotation_translation();
            *t = Transform {
                translation,
                rotation,
                scale,
            };
        }
    }
}

fn shift(g: GlobalTransform, origin: Vec3) -> GlobalTransform {
    let mut a = g.affine();
    a.translation += Vec3A::from(origin);
    GlobalTransform::from(a)
}

fn rig_worlds(
    rig: &RigPose,
    root_at_origin: GlobalTransform,
    cam: Option<(Vec3, Vec3, Vec3)>,
) -> (Vec<GlobalTransform>, Vec<bool>) {
    let n = rig.locals.len();
    let mut worlds = Vec::with_capacity(n);
    let mut in_replaced_subtree = vec![false; n];
    for i in 0..n {
        let parent = usize::try_from(rig.parents[i]).ok().filter(|&p| p < i);
        let parent_world = match parent {
            Some(p) => worlds[p],
            None => root_at_origin,
        };
        let mut g = match rig.arms[i] {
            Some(arm) => {
                in_replaced_subtree[i] = true;
                GlobalTransform::from(parent_arm_matrix(
                    arm,
                    parent_world.affine(),
                    root_at_origin.affine(),
                    rig.rest_pivots[i],
                ))
            }
            None => parent_world,
        }
        .mul_transform(rig.locals[i]);
        if let (Some(kind), Some((fwd, right, up))) = (rig.kinds[i], cam) {
            let (scale, rot, translation) = g.to_scale_rotation_translation();
            g = GlobalTransform::from(Transform {
                translation,
                rotation: billboard_basis(kind, rot, fwd, right, up),
                scale,
            });
            in_replaced_subtree[i] = true;
        } else if !in_replaced_subtree[i] {
            in_replaced_subtree[i] = parent.is_some_and(|p| in_replaced_subtree[p]);
        }
        worlds.push(g);
    }
    (worlds, in_replaced_subtree)
}

fn at_origin(root: GlobalTransform) -> GlobalTransform {
    let mut a = root.affine();
    a.translation = Vec3A::ZERO;
    GlobalTransform::from(a)
}

pub(crate) fn seed_rig_rows(
    rig: &RigPose,
    root: GlobalTransform,
    skin: &RigSkin,
    palettes: &mut RigPalettes,
) {
    let (worlds, _) = rig_worlds(rig, at_origin(root), None);
    palettes.write_rig_worlds(skin, &worlds, root.translation());
}

#[allow(clippy::type_complexity)]
fn finalize_rig_worlds(
    cam: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    mut rigs: Query<'_, '_, (Entity, &mut RigPose, Option<&RigSkin>, Has<AnimParked>)>,
    mut worlds_params: ParamSet<
        '_,
        '_,
        (
            Query<'_, '_, (), Changed<GlobalTransform>>,
            Query<'_, '_, (&Transform, &mut GlobalTransform), Without<WorldCamera>>,
        ),
    >,
    children: Query<'_, '_, &Children>,
    mut palettes: ResMut<'_, RigPalettes>,
) {
    let cam_basis = cam
        .single()
        .ok()
        .map(|t| (*t.forward(), *t.right(), *t.up()));
    let refresh: Vec<Entity> = {
        let roots_changed = worlds_params.p0();
        rigs.iter()
            .filter(|(_, rig, _, parked)| {
                !parked
                    && (rig.pose_dirty
                        || roots_changed.contains(rig.joints_root)
                        || rig.has_billboard)
            })
            .map(|(holder, ..)| holder)
            .collect()
    };
    let mut globals = worlds_params.p1();
    for holder in refresh {
        let Ok((_, rig, skin, _)) = rigs.get_mut(holder) else {
            continue;
        };
        let rig = rig.into_inner();
        rig.pose_dirty = false;
        if skin.is_none() && !rig.has_special {
            continue;
        }
        let Ok(root_g) = globals.get(rig.joints_root).map(|(_, g)| *g) else {
            continue;
        };
        let origin = root_g.translation();
        let (worlds, in_replaced_subtree) = rig_worlds(rig, at_origin(root_g), cam_basis);
        if let Some(skin) = skin {
            palettes.write_rig_worlds(skin, &worlds, origin);
        }
        if !rig.has_special {
            continue;
        }
        let mut stack: Vec<(Entity, GlobalTransform)> = Vec::new();
        for &(bone, anchor) in &rig.anchors {
            let b = bone as usize;
            if !in_replaced_subtree.get(b).copied().unwrap_or(false) {
                continue;
            }
            let Some(world) = worlds.get(b).map(|&w| shift(w, origin)) else {
                continue;
            };
            if let Ok((_, mut g)) = globals.get_mut(anchor) {
                *g = world;
            }
            if let Ok(cs) = children.get(anchor) {
                stack.extend(cs.iter().map(|c| (c, world)));
            }
        }
        while let Some((e, parent_g)) = stack.pop() {
            let Ok((local, mut global)) = globals.get_mut(e) else {
                continue;
            };
            let g = parent_g.mul_transform(*local);
            *global = g;
            if let Ok(cs) = children.get(e) {
                stack.extend(cs.iter().map(|c| (c, g)));
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.configure_sets(
        PostUpdate,
        PosePost
            .after(bevy::app::AnimationSystems)
            .before(bevy::transform::TransformSystems::Propagate),
    )
    .add_systems(
        PostUpdate,
        (
            compose_rig_models
                .after(PosePost)
                .before(bevy::transform::TransformSystems::Propagate),
            finalize_rig_worlds
                .in_set(RigFinalize)
                .after(bevy::transform::TransformSystems::Propagate),
        ),
    );
}

#[cfg(test)]
mod tests {
    use model::{ParentArm, ParentBasis};

    use super::*;
    use crate::rig::bake::{ModelJoint, ModelSkeleton};

    fn skeleton(joints: Vec<ModelJoint>) -> ModelSkeleton {
        ModelSkeleton {
            joints,
            spine_bone: None,
            head_bone: None,
        }
    }

    fn joint(parent: i16, t: Vec3) -> ModelJoint {
        ModelJoint {
            parent,
            local_translation: t,
            billboard: None,
            parent_arm: None,
        }
    }

    #[test]
    fn a_root_basis_seat_keeps_the_models_orientation_and_rides_the_spine() {
        let sk = skeleton(vec![
            joint(-1, Vec3::ZERO),
            joint(0, Vec3::Y),
            ModelJoint {
                parent: 1,
                local_translation: Vec3::new(0.0, 0.5, 0.25),
                billboard: None,
                parent_arm: Some(ParentArm {
                    ignore_translate: false,
                    basis: ParentBasis::RootBasis,
                }),
            },
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        for step in 0..16 {
            let swing = (step as f32 - 7.5) * 0.05;
            rig.locals[1].rotation = Quat::from_rotation_x(swing);
            rig.compose();
            let (_, seat_rot, seat_pos) = rig.model[2].to_scale_rotation_translation();
            assert!(seat_rot.angle_between(Quat::IDENTITY) < 1e-4, "{swing}");
            let want = rig.model[1].transform_point3(Vec3::new(0.0, 0.5, 0.25));
            assert!((seat_pos - want).length() < 1e-4, "{swing}");
        }
    }

    #[test]
    fn compose_is_the_chained_global_transform() {
        let sk = skeleton(vec![
            joint(-1, Vec3::new(1.0, 2.0, 3.0)),
            joint(0, Vec3::Y),
            joint(1, Vec3::X),
        ]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        rig.locals[0].rotation = Quat::from_rotation_y(0.7);
        rig.locals[1].scale = Vec3::new(2.0, 1.0, 0.5);
        rig.locals[1].rotation = Quat::from_rotation_x(-0.3);
        rig.compose();
        let mut g = GlobalTransform::IDENTITY;
        for i in 0..3 {
            g = g.mul_transform(rig.locals[i]);
            assert!(Mat4::from(g.affine()).abs_diff_eq(Mat4::from(rig.model[i]), 1e-5));
        }
    }

    #[test]
    fn a_plain_chain_is_the_root_times_the_model_frames() {
        let sk = skeleton(vec![joint(-1, Vec3::X), joint(0, Vec3::Y)]);
        let mut rig = RigPose::new(Entity::PLACEHOLDER, &sk);
        rig.locals[0].rotation = Quat::from_rotation_z(0.4);
        rig.compose();
        let root = GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 7.0));
        let (worlds, touched) = rig_worlds(&rig, root, None);
        assert_eq!(touched, vec![false; 2]);
        for (i, w) in worlds.iter().enumerate() {
            let expect = GlobalTransform::from(root.affine() * rig.model[i]);
            assert!((w.translation() - expect.translation()).length() < 1e-5);
        }
    }
}
