use bevy::math::{Affine3A, Mat3, Mat3A, Vec3A};
use bevy::prelude::*;
use model::{BillboardKind, ParentArm, ParentBasis};

use crate::model::BillboardInfo;
use crate::view::WorldCamera;

#[derive(Component)]
#[require(Transform, Visibility)]
pub(crate) struct BillboardCard {
    world_pivot: Vec3,
    scale: Vec3,
    kind: BillboardKind,
    placement_rot: Quat,
    placed: bool,
}

impl BillboardCard {
    pub(crate) fn new(info: &BillboardInfo, placement: &Transform) -> Self {
        Self {
            world_pivot: placement.transform_point(info.pivot),
            scale: placement.scale,
            kind: info.kind,
            placement_rot: placement.rotation,
            placed: false,
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

/// Writes each card's world transform after propagation, so the frame draws what it computes.
pub(crate) fn face_billboards(
    camera: Query<'_, '_, Ref<'_, GlobalTransform>, With<WorldCamera>>,
    mut cards: Query<
        '_,
        '_,
        (&mut BillboardCard, &mut Transform, &mut GlobalTransform),
        Without<WorldCamera>,
    >,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let moved = cam.is_changed();
    let (fwd, right, up) = (*cam.forward(), *cam.right(), *cam.up());
    for (mut card, mut tf, mut global) in &mut cards {
        if card.placed && !moved {
            continue;
        }
        card.placed = true;
        let placed = Transform {
            translation: card.world_pivot,
            rotation: billboard_basis(card.kind, card.placement_rot, fwd, right, up),
            scale: card.scale,
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
}
