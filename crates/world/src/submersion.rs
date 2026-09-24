//! Which liquid the camera's eye is under.

use bevy::camera::Projection;
use bevy::prelude::*;
use light::Submersion;
use terrain::LiquidKind;

use crate::collision::{LiquidClaim, Liquids};
use crate::coords::bevy_to_wow;
use crate::portal::CameraInteriorClaim;
use crate::view::WorldCamera;

const WATER_SUBMERSION_MARGIN: f32 = 0.01;

/// The liquid the eye is under, `Dry` when none.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Underwater(pub Submersion);

fn submersion_of(kind: LiquidKind) -> Submersion {
    match kind {
        LiquidKind::Still | LiquidKind::Rapids => Submersion::Water,
        LiquidKind::Ocean => Submersion::Ocean,
        LiquidKind::Magma => Submersion::Magma,
        LiquidKind::Slime => Submersion::Slime,
    }
}

fn lowest_near_corner_drop(rotation: Quat, fov: f32, aspect: f32, near: f32) -> f32 {
    let half_h = (fov * 0.5).tan() * near;
    let half_w = half_h * aspect;
    let mut drop: f32 = 0.0;
    for sx in [-1.0, 1.0] {
        for sy in [-1.0, 1.0] {
            drop = drop.min((rotation * Vec3::new(sx * half_w, sy * half_h, -near)).y);
        }
    }
    drop
}

pub(crate) fn detect_submersion(
    mut underwater: ResMut<'_, Underwater>,
    camera: Query<'_, '_, (&Transform, &Projection), With<WorldCamera>>,
    liquids: Liquids<'_, '_>,
    eye_claim: Option<Res<'_, CameraInteriorClaim>>,
) {
    let Ok((cam, projection)) = camera.single() else {
        return;
    };
    let claim = if eye_claim.is_some_and(|c| c.0.is_some()) {
        LiquidClaim::Inside
    } else {
        LiquidClaim::Outdoors
    };
    let eye = bevy_to_wow(cam.translation);
    let near_plane_bottom_z = match projection {
        Projection::Perspective(p) => {
            eye[2] + lowest_near_corner_drop(cam.rotation, p.fov, p.aspect_ratio, p.near)
        }
        _ => eye[2],
    };
    let verdict = liquids
        .surfaces_at([eye[0], eye[1], near_plane_bottom_z], claim)
        .into_iter()
        .filter_map(|hit| {
            let eps = if hit.kind.is_fullbright() {
                0.0
            } else {
                WATER_SUBMERSION_MARGIN
            };
            (near_plane_bottom_z < hit.surface_z + eps)
                .then_some((hit.surface_z, submersion_of(hit.kind)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Submersion::Dry, |(_, s)| s);
    underwater.set_if_neq(Underwater(verdict));
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_is_the_near_rectangles_lowest_corner() {
        let (fov, aspect, near) = (std::f32::consts::FRAC_PI_4, 16.0 / 9.0, 1.0 / 9.0);
        let half_h = (fov * 0.5).tan() * near;
        let at = |yaw: f32, pitch: f32| {
            lowest_near_corner_drop(
                Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0),
                fov,
                aspect,
                near,
            )
        };
        assert!((at(0.0, 0.0) + half_h).abs() < 1e-6);
        assert!((at(0.0, -std::f32::consts::FRAC_PI_2) + near).abs() < 1e-6);
        assert_eq!(at(0.0, std::f32::consts::FRAC_PI_2), 0.0);
        assert!((at(1.23, -0.4) - at(0.0, -0.4)).abs() < 1e-6);
    }
}
