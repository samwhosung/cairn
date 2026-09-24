//! Which liquid the camera's eye is under.

use bevy::camera::Projection;
use bevy::prelude::*;
use light::Submersion;

use crate::coords::bevy_to_wow;
use crate::liquid::{LiquidClaim, LiquidGrid, submersion_claim_at};
use crate::portal::{CameraInteriorClaim, WmoPortalInstance};
use crate::view::WorldCamera;
use crate::wmo::WmoModel;

/// The liquid the eye is under, `Dry` when none.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Underwater(pub Submersion);

#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub struct SubmergedEye {
    /// Yards from the probe up to the surface over it; 0 when dry.
    pub depth: f32,
    /// The eye's own WoW Z: the ocean darkens by it.
    pub eye_z: f32,
}

/// Where [`Underwater`] is written each frame; what reads it orders itself after.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubmersionVerdict;

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

fn camera_claim(
    claim: &CameraInteriorClaim,
    instances: &Query<'_, '_, &WmoPortalInstance>,
    wmos: &Assets<WmoModel>,
) -> LiquidClaim {
    let Some(room) = claim.0 else {
        return LiquidClaim::Outdoors;
    };
    let nav = instances
        .get(room.instance)
        .ok()
        .and_then(|inst| wmos.get(&inst.handle))
        .map_or(&[][..], |m| m.rooms.group_nav.as_slice());
    LiquidClaim::inside(room, nav)
}

pub(crate) fn detect_submersion(
    camera: Query<'_, '_, (&Transform, &Projection), With<WorldCamera>>,
    grids: Query<'_, '_, &LiquidGrid>,
    claim: Res<'_, CameraInteriorClaim>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    wmos: Res<'_, Assets<WmoModel>>,
    mut underwater: ResMut<'_, Underwater>,
    mut eye: ResMut<'_, SubmergedEye>,
) {
    let Ok((cam, projection)) = camera.single() else {
        return;
    };
    let claim = camera_claim(&claim, &instances, &wmos);
    let at = bevy_to_wow(cam.translation);
    let probe_z = match projection {
        Projection::Perspective(p) => {
            at[2] + lowest_near_corner_drop(cam.rotation, p.fov, p.aspect_ratio, p.near)
        }
        _ => at[2],
    };
    let verdict = submersion_claim_at(grids.iter(), [at[0], at[1], probe_z], claim);
    underwater.set_if_neq(Underwater(verdict.map(|(s, _)| s).unwrap_or_default()));
    eye.set_if_neq(SubmergedEye {
        depth: verdict.map_or(0.0, |(_, z)| (z - probe_z).max(0.0)),
        eye_z: at[2],
    });
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
