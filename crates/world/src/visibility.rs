use std::num::NonZeroU16;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::LeftOut;
use crate::doodad_anim::MatAnim;
use crate::liquid::{FarSide, LiquidGrid};
use crate::model_material::ModelMaterial;
use crate::portal::{WmoGroupVis, WmoPortalInstance};
use crate::sight::Meetable;
use crate::view::{WorldCamera, nearest_depth_within_farclip};

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
const SHADE_OR_PROBE_MASK: u32 = 0x1fff << PROBE_SHIFT;
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

/// A sight index goes in the shade and probe bits, which light, and the fog and highlight bits.
const SIGHT_LOW_BITS: u32 = 13;
const SIGHT_HIGH_SHIFT: u32 = 30;
/// How many placements a sight frame can tell apart.
pub(crate) const SIGHT_INDICES: u32 = 1 << (SIGHT_LOW_BITS + 2);

/// A batch's tag for its sight twin: the batch's fade and rig, and the index of its placement.
pub(crate) fn with_sight_index(tag: u32, index: u32) -> u32 {
    let low = (index & ((1 << SIGHT_LOW_BITS) - 1)) << PROBE_SHIFT;
    let high = (index >> SIGHT_LOW_BITS) << SIGHT_HIGH_SHIFT;
    (tag & (ALPHA_MASK | RIG_MASK)) | low | high
}

pub(crate) fn with_alpha(tag: u32, alpha: f32) -> u32 {
    (tag & !ALPHA_MASK) | alpha_bits(alpha)
}

pub(crate) fn with_rig(tag: u32, slot: Option<NonZeroU16>) -> u32 {
    (tag & !RIG_MASK) | (u32::from(slot.map_or(0, NonZeroU16::get)) << RIG_SHIFT)
}

pub(crate) fn with_shade_or_probe(tag: u32, shade_or_probe: u16) -> u32 {
    (tag & !SHADE_OR_PROBE_MASK)
        | ((u32::from(shade_or_probe) << PROBE_SHIFT) & SHADE_OR_PROBE_MASK)
}

pub(crate) fn translucent(tag: u32) -> bool {
    let payload = tag & !INTERIOR_FOG_BIT;
    payload != 0 && (1..ALPHA_MASK).contains(&(payload & ALPHA_MASK))
}

pub(crate) fn with_interior_fog(tag: u32, on: bool) -> u32 {
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

fn apply_pool_visibility(
    instances: &Query<'_, '_, &WmoPortalInstance>,
    pools: &mut Query<'_, '_, Pool<'_>, (With<LiquidGrid>, Without<ModelPart>)>,
) {
    for (room, mut vis, mut tag) in pools {
        let inst = instances.get(room.instance).ok();
        let visited = inst.is_none_or(|inst| {
            room.groups
                .iter()
                .any(|&g| inst.ever_flooded.get(g as usize).copied().unwrap_or(true))
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
        let in_range = nearest_depth_within_farclip(cam_pos, cam_fwd, center, radius);
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
            let want = side.sided(
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

/// The render layer a left-out placement's batches go to, which no camera draws: they stay visible,
/// so they go on animating as if seen.
const LEFT_OUT_LAYER: usize = 30;

#[derive(Component)]
pub(crate) struct LeftOutPart;

/// Moves the batches of the placements [`LeftOut`] names out of the world camera's layer, and back.
pub(crate) fn leave_out(
    mut commands: Commands<'_, '_>,
    left_out: Res<'_, LeftOut>,
    parts: Query<'_, '_, (Entity, &Meetable, Has<LeftOutPart>)>,
    arrived: Query<'_, '_, (), Added<Meetable>>,
) {
    if !left_out.is_changed() && (left_out.0.is_empty() || arrived.is_empty()) {
        return;
    }
    for (part, met, out) in &parts {
        let wanted = met
            .seen
            .placement()
            .is_some_and(|id| left_out.0.contains(&id));
        if wanted && !out {
            commands
                .entity(part)
                .insert((LeftOutPart, RenderLayers::layer(LEFT_OUT_LAYER)));
        } else if out && !wanted {
            commands
                .entity(part)
                .remove::<(LeftOutPart, RenderLayers)>();
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
    fn a_sight_twin_keeps_its_batchs_fade_and_rig_and_carries_its_placements_index() {
        let batch = INTERIOR_FOG_BIT | 0x0015_0000 | (6660 << PROBE_SHIFT) | alpha_bits(0.5);
        for index in [0, 1, 2, 8191, 8192, SIGHT_INDICES - 1] {
            let twin = with_sight_index(batch, index);
            assert_eq!(
                twin & (ALPHA_MASK | RIG_MASK),
                batch & (ALPHA_MASK | RIG_MASK)
            );
            let read = ((twin >> PROBE_SHIFT) & 0x1fff) | ((twin >> 30) << 13);
            assert_eq!(read, index, "as the shader reads it back");
        }
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

    #[test]
    fn a_named_building_without_portals_draws_its_pools() {
        use std::sync::Arc;

        use crate::adt::AdtTile;
        use crate::liquid::{LiquidSource, WmoPool};
        use crate::portal::{
            CameraInteriorClaim, ExteriorWindows, WholeBuildings, compute_wmo_pvs,
        };
        use crate::room::CameraRoom;
        use crate::stream::Streamer;
        use crate::wmo::{WmoGroupNav, WmoModel, WmoRooms};

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()))
            .init_asset::<WmoModel>()
            .init_asset::<AdtTile>()
            .init_resource::<Streamer>()
            .init_resource::<CameraRoom>()
            .init_resource::<CameraInteriorClaim>()
            .init_resource::<ExteriorWindows>()
            .init_resource::<WholeBuildings>()
            .init_resource::<FarSide>()
            .add_systems(Update, (compute_wmo_pvs, apply_model_visibility).chain());
        let rooms = WmoRooms {
            wmo_id: 63,
            group_nav: vec![WmoGroupNav {
                flags: crate::portal::EXTERIOR,
                wmo_group_id: 0,
                bbox_min: [0.0; 3],
                bbox_max: [4.0; 3],
                ref_start: 0,
                ref_count: 0,
                interior: false,
                flooded: None,
                fog_indices: [0; 4],
            }],
            ..WmoRooms::default()
        };
        let instance = app.world_mut().spawn_empty().id();
        let basin = LiquidGrid::new(
            LiquidSource::WmoGroup(WmoPool::of(&rooms, 0, instance, &Transform::IDENTITY)),
            terrain::LiquidKind::Still,
            [2, 2],
            vec![
                [0.0, 0.0, 1.0],
                [4.0, 0.0, 1.0],
                [0.0, 4.0, 1.0],
                [4.0, 4.0, 1.0],
            ],
            vec![true],
        );
        let model = app
            .world_mut()
            .resource_mut::<Assets<WmoModel>>()
            .add(WmoModel {
                rooms,
                ..WmoModel::default()
            });
        app.world_mut()
            .entity_mut(instance)
            .insert(WmoPortalInstance::new(model, &Transform::IDENTITY, 1, 0));
        let pool = app
            .world_mut()
            .spawn((
                WmoGroupVis {
                    instance,
                    groups: Arc::from([0].as_slice()),
                },
                basin,
                Visibility::Hidden,
                MeshTag(0),
            ))
            .id();
        app.world_mut().spawn((
            WorldCamera,
            GlobalTransform::IDENTITY,
            Projection::default(),
        ));
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(pool),
            Some(&Visibility::Inherited)
        );
    }

    #[test]
    fn a_shade_or_probe_write_leaves_the_alpha_the_rig_and_the_fog_alone() {
        let rig = 0x3ff8_0000;
        let t = with_shade_or_probe(INTERIOR_FOG_BIT | rig | alpha_bits(0.5), 6660);
        assert_eq!((t >> PROBE_SHIFT) & 0x1fff, 6660);
        assert_eq!(
            t & !SHADE_OR_PROBE_MASK,
            INTERIOR_FOG_BIT | rig | alpha_bits(0.5)
        );
        assert_eq!(with_shade_or_probe(t, 0) & SHADE_OR_PROBE_MASK, 0);
    }
}
