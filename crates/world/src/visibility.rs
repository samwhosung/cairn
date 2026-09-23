use std::num::NonZeroU16;

use bevy::camera::primitives::Aabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::doodad_anim::MatAnim;
use crate::liquid::{FarSide, LiquidGrid};
use crate::model_material::ModelMaterial;
use crate::portal::{WmoGroupVis, WmoPortalInstance};
use crate::view::{FARCLIP, WorldCamera};

const NEVER_FADE_RADIUS: f32 = 7.0;

struct FadeBand {
    max_radius: f32,
    start: f32,
    range: f32,
}

const FADE_BANDS: [FadeBand; 3] = [
    FadeBand {
        max_radius: 0.5,
        start: 40.0,
        range: 10.0,
    },
    FadeBand {
        max_radius: 2.5,
        start: 100.0,
        range: 25.0,
    },
    FadeBand {
        max_radius: NEVER_FADE_RADIUS,
        start: 150.0,
        range: 50.0,
    },
];

const ALPHA_MASK: u32 = 0x3f;
const ALPHA_MAX: f32 = 63.0;
const PROBE_SHIFT: u32 = 6;
const RIG_MASK: u32 = 0x3ff8_0000;
const RIG_SHIFT: u32 = 19;
const INTERIOR_FOG_BIT: u32 = 0x4000_0000;

/// A doodad's distance fade: opaque until `start` yards past its bounding sphere, measured across
/// the ground to its centre, then linear to nothing over `range`. The radius picks the band.
pub(crate) fn doodad_fade_alpha(radius: f32, horiz_dist: f32) -> f32 {
    if radius > NEVER_FADE_RADIUS {
        return 1.0;
    }
    let d = horiz_dist - radius;
    let band = FADE_BANDS
        .iter()
        .find(|b| radius <= b.max_radius)
        .unwrap_or(&FADE_BANDS[2]);
    (1.0 - (d - band.start) / band.range).clamp(0.0, 1.0)
}

pub(crate) fn alpha_bits(alpha: f32) -> u32 {
    if alpha <= 0.0 {
        1
    } else {
        ((alpha.min(1.0) * ALPHA_MAX).round() as u32).max(1)
    }
}

pub(crate) fn probe_bits(slot: u16) -> u32 {
    INTERIOR_FOG_BIT | (u32::from(slot) << PROBE_SHIFT) | alpha_bits(1.0)
}

pub(crate) fn with_alpha(tag: u32, alpha: f32) -> u32 {
    (tag & !ALPHA_MASK) | alpha_bits(alpha)
}

pub(crate) fn with_rig(tag: u32, slot: Option<NonZeroU16>) -> u32 {
    (tag & !RIG_MASK) | (u32::from(slot.map_or(0, NonZeroU16::get)) << RIG_SHIFT)
}

pub(crate) fn translucent(tag: u32) -> bool {
    let payload = tag & !INTERIOR_FOG_BIT;
    payload != 0 && (1..ALPHA_MASK).contains(&(payload & ALPHA_MASK))
}

fn with_interior_fog(tag: u32, on: bool) -> u32 {
    if on {
        tag | INTERIOR_FOG_BIT
    } else {
        tag & !INTERIOR_FOG_BIT
    }
}

#[derive(Component, Clone, Copy)]
pub(crate) struct ModelPart;

#[derive(Component, Clone)]
pub(crate) struct DoodadFade {
    pub radius: f32,
    pub local_center: Vec3,
    pub cutout: Handle<ModelMaterial>,
    pub blend: Handle<ModelMaterial>,
}

type Part<'a> = (
    Entity,
    &'a ModelPart,
    &'a GlobalTransform,
    &'a mut Visibility,
    Option<&'a DoodadFade>,
    &'a mut MeshTag,
    &'a mut MeshMaterial3d<ModelMaterial>,
    Option<&'a Aabb>,
    Option<&'a WmoGroupVis>,
    Option<&'a MatAnim>,
);

type Pool<'a> = (&'a WmoGroupVis, &'a mut Visibility, &'a mut MeshTag);

/// A building's pool draws once the flood has reached its group, from then on; it fogs with its
/// room while the room is on the interior fog chain.
fn apply_pool_visibility(
    instances: &Query<'_, '_, &WmoPortalInstance>,
    pools: &mut Query<'_, '_, Pool<'_>, (With<LiquidGrid>, Without<ModelPart>)>,
) {
    for (room, mut vis, mut tag) in pools {
        let inst = instances.get(room.instance).ok();
        let visited = inst.is_none_or(|inst| {
            room.groups
                .iter()
                .any(|&g| inst.liquid_visited.get(g as usize).copied().unwrap_or(true))
        });
        let want = if visited {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        let bits = with_interior_fog(tag.0, inst.is_some_and(|i| room.interior_fogged_by(i)));
        if tag.0 != bits {
            tag.0 = bits;
        }
    }
}

pub(crate) fn apply_model_visibility(
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    side: Res<'_, FarSide>,
    mut parts: Query<'_, '_, Part<'_>>,
    mut pools: Query<'_, '_, Pool<'_>, (With<LiquidGrid>, Without<ModelPart>)>,
) {
    apply_pool_visibility(&instances, &mut pools);
    let Ok(cam) = camera.single() else {
        return;
    };
    let (cam_pos, cam_fwd) = (cam.translation(), *cam.forward());
    for (entity, _, xf, mut vis, fade, mut tag, mut material, aabb, group_vis, mat) in &mut parts {
        let (center, radius) = match aabb {
            Some(a) => (
                xf.transform_point(Vec3::from(a.center)),
                Vec3::from(a.half_extents).length() * xf.affine().matrix3.x_axis.length(),
            ),
            None => (xf.translation(), 0.0),
        };
        let in_range = (center - cam_pos).dot(cam_fwd) - radius <= FARCLIP;
        let fade_alpha = fade.map_or(1.0, |f| {
            let c = xf.transform_point(f.local_center);
            let (dx, dz) = (c.x - cam_pos.x, c.z - cam_pos.z);
            doodad_fade_alpha(f.radius, (dx * dx + dz * dz).sqrt())
        });
        let instance = group_vis.and_then(|gv| instances.get(gv.instance).ok());
        let portal_visible = group_vis.is_none_or(|gv| instance.is_none_or(|i| gv.drawn_by(i)));
        let room_fog = group_vis.map(|gv| instance.is_some_and(|i| gv.interior_fogged_by(i)));
        let mat_factor = mat.map_or(1.0, |m| m.alpha);
        let desired = if in_range && fade_alpha > 0.0 && mat_factor > 0.0 && portal_visible {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != desired {
            *vis = desired;
        }
        let mut bits = tag.0;
        if fade.is_some() || mat.is_some() {
            bits = with_alpha(bits, fade_alpha * mat_factor);
        }
        if let Some(on) = room_fog {
            bits = with_interior_fog(bits, on);
        }
        if tag.0 != bits {
            tag.0 = bits;
        }
        if let Some(f) = fade {
            let want = side.resolve(
                entity,
                if fade_alpha < 1.0 {
                    &f.blend
                } else {
                    &f.cutout
                },
            );
            if material.0 != *want {
                material.0 = want.clone();
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn the_band_is_chosen_by_size() {
        assert_eq!(doodad_fade_alpha(8.0, 10_000.0), 1.0);
        assert_eq!(doodad_fade_alpha(0.5, 40.5), 1.0);
        assert!((doodad_fade_alpha(0.5, 45.5) - 0.5).abs() < 1e-6);
        assert_eq!(doodad_fade_alpha(0.5, 50.5), 0.0);
        assert!((doodad_fade_alpha(2.0, 114.5) - 0.5).abs() < 1e-6);
        assert!((doodad_fade_alpha(7.0, 182.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_tag_keeps_its_slot_through_a_fade() {
        let t = probe_bits(6660);
        assert_eq!((t >> PROBE_SHIFT) & 0x1fff, 6660);
        assert_eq!(t & ALPHA_MASK, 63);
        let t = with_alpha(t, 0.25);
        assert_eq!((t >> PROBE_SHIFT) & 0x1fff, 6660);
        assert_eq!(t & ALPHA_MASK, alpha_bits(0.25));
        assert_eq!(alpha_bits(0.0), 1);
        assert_eq!(with_interior_fog(t, false) & INTERIOR_FOG_BIT, 0);
    }
}
