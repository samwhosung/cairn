use bevy::mesh::MeshTag;
use bevy::prelude::*;

use super::batch_anim::{UnitCards, unit_parts};
use super::light::UnitLight;
use crate::adt::AdtTile;
use crate::model_material::GroundShade;
use crate::models::ground_shade;
use crate::stream::Streamer;
use crate::visibility::with_shade_or_probe;

/// Where the mix sits between the lit and the shadowed intensity: 0 is the client's lit 2.5, 1
/// its shadowed 0.5. The shader caps the intensity at 1, so the lit end is aimed at the mix that
/// shows as 1, 0.75.
const LIT_T: f32 = 0.75;
const SHADOWED_T: f32 = 1.0;
const LIT_INTENSITY: f32 = 2.5;
const SHADOWED_INTENSITY: f32 = 0.5;
const INDOOR_INTENSITY: f32 = 1.0;
const INDOOR_T: f32 = mix_for(INDOOR_INTENSITY);
/// The client ramps the intensity at 3.3333 a second over the 2.0-wide span.
const RAMP_PER_SEC: f32 = 3.3333 / 2.0;
const AMBIENT_RAMP_PER_SEC: f32 = 2.0;
const RESAMPLE_DIST: f32 = 0.5;
const SETTLED_EPS: f32 = 0.4 / 255.0;

/// A unit's own light: the baked ground shadow under its feet, ramped as it walks in and out of
/// it, and carried to its parts unless a probe of its own lights them. Indoors it aims at an
/// intensity of 1 and ramps the ambient colour its probe is folded from; on a building's outdoor
/// surface it stays lit.
#[derive(Component)]
pub struct UnitShade {
    t: f32,
    target: f32,
    last_pos: Vec3,
    sampled: bool,
    pub(crate) indoor: bool,
    pub(crate) on_building_outdoors: bool,
    pub(crate) ambient: Vec3,
    pub(crate) ambient_target: Vec3,
}

impl Default for UnitShade {
    fn default() -> Self {
        Self {
            t: LIT_T,
            target: LIT_T,
            last_pos: Vec3::ZERO,
            sampled: false,
            indoor: false,
            on_building_outdoors: false,
            ambient: Vec3::ZERO,
            ambient_target: Vec3::ZERO,
        }
    }
}

impl UnitShade {
    pub(crate) fn tag_byte(&self) -> u8 {
        (self.t * 255.0).round().clamp(0.0, 255.0) as u8
    }

    pub(crate) fn intensity(&self) -> f32 {
        LIT_INTENSITY + (SHADOWED_INTENSITY - LIT_INTENSITY) * self.t
    }

    fn effective_target(&self) -> f32 {
        if self.indoor {
            INDOOR_T
        } else if self.on_building_outdoors {
            LIT_T
        } else {
            self.target
        }
    }

    pub(crate) fn ramps_settled(&self) -> bool {
        (self.t - self.effective_target()).abs() < SETTLED_EPS
            && (self.ambient - self.ambient_target).abs().max_element() < SETTLED_EPS
    }

    pub(crate) fn seed_ambient(&mut self, scene: Vec3, room: Vec3) {
        self.ambient = scene;
        self.ambient_target = room;
    }
}

const fn mix_for(intensity: f32) -> f32 {
    (LIT_INTENSITY - intensity) / (LIT_INTENSITY - SHADOWED_INTENSITY)
}

fn ramp_toward(v: f32, target: f32, step: f32) -> f32 {
    if v < target {
        (v + step).min(target)
    } else {
        (v - step).max(target)
    }
}

type Units<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static mut UnitShade,
        Option<&'static UnitLight>,
        Option<&'static UnitCards>,
    ),
>;

pub(crate) fn update_unit_shade(
    time: Res<'_, Time>,
    streamer: Res<'_, Streamer>,
    adts: Res<'_, Assets<AdtTile>>,
    mut units: Units<'_, '_>,
    children: Query<'_, '_, &Children>,
    mut tags: Query<'_, '_, &mut MeshTag>,
) {
    let step = RAMP_PER_SEC * time.delta_secs();
    let ambient_step = AMBIENT_RAMP_PER_SEC * time.delta_secs();
    for (root, gt, mut shade, light, cards) in &mut units {
        let pos = gt.translation();
        if !shade.sampled || pos.distance(shade.last_pos) >= RESAMPLE_DIST {
            let at = Transform::from_translation(pos);
            if let Some(ground) = ground_shade(&streamer, &adts, &at) {
                shade.target = if ground == GroundShade::Shadowed {
                    SHADOWED_T
                } else {
                    LIT_T
                };
                shade.last_pos = pos;
                if !shade.sampled {
                    shade.t = shade.effective_target();
                    shade.sampled = true;
                }
            }
        }
        let target = shade.effective_target();
        shade.t = ramp_toward(shade.t, target, step);
        let (a, at) = (shade.ambient, shade.ambient_target);
        shade.ambient = Vec3::new(
            ramp_toward(a.x, at.x, ambient_step),
            ramp_toward(a.y, at.y, ambient_step),
            ramp_toward(a.z, at.z, ambient_step),
        );
        if light.is_some_and(|l| l.probe_lit()) {
            continue;
        }
        let byte = u16::from(shade.tag_byte());
        for part in unit_parts(root, &children, cards) {
            if let Ok(mut tag) = tags.get_mut(part) {
                let bits = with_shade_or_probe(tag.0, byte);
                if tag.0 != bits {
                    tag.0 = bits;
                }
            }
        }
    }
}
