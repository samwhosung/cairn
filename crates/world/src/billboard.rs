use bevy::math::{Affine3A, Mat3, Mat3A, Vec3A};
use bevy::prelude::*;
use model::{BillboardKind, BoneScaleAnim, ParentArm, ParentBasis};

use crate::model::BillboardInfo;
use crate::view::WorldCamera;

#[derive(Component)]
#[require(Transform, Visibility)]
pub(crate) struct BillboardCard {
    kind: BillboardKind,
    anchor: CardAnchor,
    placed_at_ms: Option<u32>,
}

enum CardAnchor {
    Fixed {
        pivot: Vec3,
        rotation: Quat,
        scale: Vec3,
        pulse: Option<BoneScaleAnim>,
    },
    Joint(Entity),
}

impl BillboardCard {
    pub(crate) fn new(info: &BillboardInfo, placement: &Transform) -> Self {
        Self {
            kind: info.kind,
            anchor: CardAnchor::Fixed {
                pivot: placement.transform_point(info.pivot),
                rotation: placement.rotation,
                scale: placement.scale,
                pulse: info.global_seq_scale.clone(),
            },
            placed_at_ms: None,
        }
    }

    pub(crate) fn following_joint(kind: BillboardKind, joint: Entity) -> Self {
        Self {
            kind,
            anchor: CardAnchor::Joint(joint),
            placed_at_ms: None,
        }
    }
}

/// The view's own basis, shared by every card, never an aim at the card.
pub(crate) fn billboard_basis(
    kind: BillboardKind,
    kept_rot: Quat,
    fwd: Vec3,
    right: Vec3,
    up: Vec3,
) -> Quat {
    let (bone_x, bone_y, bone_z) = match kind {
        BillboardKind::Spherical => (-fwd, right, up),
        BillboardKind::LockZ => {
            let bone_z = (kept_rot * Vec3::Y).normalize_or(Vec3::Y);
            let bone_y = fwd.cross(bone_z).try_normalize().unwrap_or(right);
            (bone_y.cross(bone_z), bone_y, bone_z)
        }
        BillboardKind::LockX => {
            let bone_x = (kept_rot * -Vec3::Z).normalize_or(-fwd);
            let bone_z = fwd.cross(bone_x).try_normalize().unwrap_or(up);
            (bone_x, bone_z.cross(bone_x), bone_z)
        }
        BillboardKind::LockY => {
            let bone_y = (kept_rot * -Vec3::X).normalize_or(right);
            let bone_x = fwd.cross(bone_y).try_normalize().unwrap_or(-fwd);
            (bone_x, bone_y, bone_x.cross(bone_y))
        }
    };
    rotation_onto_wow_axes(bone_x, bone_y, bone_z)
}

/// Unless the arm takes the root's origin, the bone stays where its animated parent carried it
/// and only the basis changes.
pub(crate) fn parent_arm_matrix(
    arm: ParentArm,
    parent: Affine3A,
    model_root: Affine3A,
    rest_pivot: Vec3,
) -> Affine3A {
    const UNIT_EPS: f32 = 1.0 / (1 << 22) as f32;
    const RATIO_EPS: f32 = 1e-5;
    let (p, r) = (parent.matrix3, model_root.matrix3);
    let per_axis = |f: &dyn Fn(usize) -> Vec3A| Mat3A::from_cols(f(0), f(1), f(2));
    let matrix3 = match arm.basis {
        ParentBasis::Keep => p,
        ParentBasis::UnitNormalize => per_axis(&|k| {
            let len = p.col(k).length();
            if len > UNIT_EPS {
                p.col(k) / len
            } else {
                p.col(k)
            }
        }),
        ParentBasis::RootDirection => per_axis(&|k| {
            let rl2 = r.col(k).length_squared();
            let ratio = if rl2 <= RATIO_EPS {
                1.0
            } else {
                p.col(k).length() / rl2.sqrt()
            };
            r.col(k) * ratio
        }),
        ParentBasis::RootBasis => r,
    };
    Affine3A {
        matrix3,
        translation: if arm.ignore_translate {
            model_root.translation
        } else {
            parent.transform_point3a(rest_pivot.into()) - matrix3 * Vec3A::from(rest_pivot)
        },
    }
}

/// The rotation of a mesh in Bevy's axes that lays its WoW x, y and z along the given directions.
fn rotation_onto_wow_axes(x: Vec3, y: Vec3, z: Vec3) -> Quat {
    Quat::from_mat3(&Mat3::from_cols(-y, z, -x))
}

type Camera<'w, 's> = Query<
    'w,
    's,
    (
        Ref<'static, GlobalTransform>,
        Option<Ref<'static, Transform>>,
    ),
    With<WorldCamera>,
>;

type Cards<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut BillboardCard,
        &'static mut Transform,
        &'static mut GlobalTransform,
    ),
    Without<WorldCamera>,
>;

type Joints<'w, 's> =
    Query<'w, 's, Ref<'static, GlobalTransform>, (Without<WorldCamera>, Without<BillboardCard>)>;

/// Writes each card's world transform after propagation, so the frame draws what it computes.
pub(crate) fn face_billboards(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    camera: Camera<'_, '_>,
    joints: Joints<'_, '_>,
    mut cards: Cards<'_, '_>,
) {
    let Ok((cam, cam_local)) = camera.single() else {
        return;
    };
    let moved = cam.is_changed() || cam_local.is_some_and(|l| l.is_changed());
    let (fwd, right, up) = (*cam.forward(), *cam.right(), *cam.up());
    let now_ms = time.elapsed().as_millis() as u32;
    for (entity, mut card, mut tf, mut global) in &mut cards {
        let first = card.placed_at_ms.is_none();
        let (pivot, rotation, scale) = match &card.anchor {
            CardAnchor::Joint(joint) => {
                let Ok(joint) = joints.get(*joint) else {
                    commands.entity(entity).try_despawn();
                    continue;
                };
                if !first && !moved && !joint.is_changed() {
                    continue;
                }
                let t = joint.compute_transform();
                (t.translation, t.rotation, t.scale)
            }
            CardAnchor::Fixed {
                pivot,
                rotation,
                scale,
                pulse,
            } => {
                if !first && !moved && pulse.is_none() {
                    continue;
                }
                // The client runs a model's global sequences from its creation.
                let age_ms = now_ms.wrapping_sub(card.placed_at_ms.unwrap_or(now_ms));
                let pulse = pulse
                    .as_ref()
                    .map_or(Vec3::ONE, |p| Vec3::from_array(p.sample(age_ms)));
                (*pivot, *rotation, *scale * pulse)
            }
        };
        if first {
            card.placed_at_ms = Some(now_ms);
        }
        let placed = Transform {
            translation: pivot,
            rotation: billboard_basis(card.kind, rotation, fwd, right, up),
            scale,
        };
        if *tf != placed {
            *tf = placed;
        }
        let placed_global = GlobalTransform::from(placed);
        if *global != placed_global {
            *global = placed_global;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coords::wow_to_bevy;

    #[test]
    fn a_spherical_card_turns_its_bone_x_to_the_viewer() {
        for (fwd, right, up) in [
            (Vec3::NEG_Z, Vec3::X, Vec3::Y),
            (Vec3::X, Vec3::Z, Vec3::Y),
            (
                Vec3::new(0.0, -0.6, -0.8),
                Vec3::X,
                Vec3::new(0.0, 0.8, -0.6),
            ),
        ] {
            let q = billboard_basis(BillboardKind::Spherical, Quat::IDENTITY, fwd, right, up);
            let bone_x = q * wow_to_bevy([1.0, 0.0, 0.0]);
            let bone_z = q * wow_to_bevy([0.0, 0.0, 1.0]);
            assert!((bone_x + fwd).length() < 1e-5, "{fwd}: {bone_x}");
            assert!((bone_z - up).length() < 1e-5, "{fwd}: {bone_z}");
        }
    }

    #[test]
    fn a_lock_z_card_keeps_its_up() {
        let tilted = Quat::from_rotation_x(0.3);
        let fwd = Vec3::new(0.2, -0.3, -0.93).normalize();
        let q = billboard_basis(BillboardKind::LockZ, tilted, fwd, Vec3::X, Vec3::Y);
        let bone_z = q * wow_to_bevy([0.0, 0.0, 1.0]);
        assert!((bone_z - tilted * Vec3::Y).length() < 1e-5);
    }

    fn info(global_seq_scale: Option<BoneScaleAnim>) -> BillboardInfo {
        BillboardInfo {
            pivot: Vec3::new(0.0, 1.7, 0.0),
            kind: BillboardKind::Spherical,
            bone: 0,
            global_seq_scale,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<Time>();
        app.add_systems(Update, face_billboards);
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::from_translation(Vec3::new(0.0, 0.0, 10.0)),
        ));
        app
    }

    #[test]
    fn a_card_on_a_joint_rides_it_and_goes_with_it() {
        let mut app = app();
        let joint = app
            .world_mut()
            .spawn(GlobalTransform::from(
                Transform::from_xyz(5.0, 0.0, 0.0).with_scale(Vec3::new(6.0, 3.0, 6.0)),
            ))
            .id();
        let card = app
            .world_mut()
            .spawn(BillboardCard::following_joint(
                BillboardKind::Spherical,
                joint,
            ))
            .id();
        app.update();
        let tf = *app.world().entity(card).get::<Transform>().expect("placed");
        assert_eq!(tf.translation, Vec3::new(5.0, 0.0, 0.0));
        assert_eq!(tf.scale, Vec3::new(6.0, 3.0, 6.0));
        app.world_mut().entity_mut(joint).despawn();
        app.update();
        assert!(app.world().get_entity(card).is_err());
    }

    #[test]
    fn a_placed_card_pulses_from_its_first_frame() {
        let mut app = app();
        let pulse = BoneScaleAnim {
            duration_ms: 1000,
            interp: true,
            keys: vec![(0, [1.0; 3]), (500, [3.0; 3]), (1000, [1.0; 3])],
        };
        let card = app
            .world_mut()
            .spawn(BillboardCard::new(&info(Some(pulse)), &Transform::IDENTITY))
            .id();
        let scale = |app: &App| {
            app.world()
                .entity(card)
                .get::<Transform>()
                .expect("a card")
                .scale
        };
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs(7));
        app.update();
        assert_eq!(
            scale(&app),
            Vec3::ONE,
            "the loop starts where the card appears"
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(250));
        app.update();
        assert!((scale(&app) - Vec3::splat(2.0)).length() < 1e-5);
    }
}
