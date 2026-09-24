use bevy::mesh::MeshTag;
use bevy::prelude::*;

use super::light::UnitLight;
use super::loops::UnitCards;
use crate::adt::AdtTile;
use crate::model_material::GroundShade;
use crate::models::ground_shade;
use crate::stream::Streamer;

/// Where the mix sits between the lit and the shadowed intensity: 0 is the client's lit 2.5, 1
/// its shadowed 0.5. The shader caps the intensity at 1, so the lit end is aimed at the mix that
/// shows as 1, 0.75.
const LIT_T: f32 = 0.75;
const SHADOWED_T: f32 = 1.0;
/// Indoors the client aims at an intensity of 1, which is where the lit end already sits.
const INDOOR_T: f32 = 0.75;
/// The client ramps the intensity at 3.3333 a second over the 2.0-wide span.
const RAMP_PER_SEC: f32 = 3.3333 / 2.0;
/// The ambient word ramps at 2.0 a second on each channel.
const AMBIENT_RAMP_PER_SEC: f32 = 2.0;
const RESAMPLE_DIST: f32 = 0.5;
/// Under half a byte of the tag or of a colour channel.
const SETTLED_EPS: f32 = 1.0 / 640.0;

const SHADE_MASK: u32 = 0x0000_3fc0;
const SHADE_SHIFT: u32 = 6;

/// A unit's own light: the baked ground shadow under its feet, ramped as it walks in and out of
/// it, carried to every part it draws. In a building it aims at the room's intensity instead, and
/// ramps the ambient colour its probe is folded from.
#[derive(Component)]
pub struct UnitShade {
    t: f32,
    target: f32,
    last_pos: Vec3,
    sampled: bool,
    pub(crate) indoor: bool,
    /// On a building's outdoor surface, a street or a deck: lit, whatever shadow the ground beneath
    /// the building holds.
    pub(crate) on_wmo: bool,
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
            on_wmo: false,
            ambient: Vec3::ZERO,
            ambient_target: Vec3::ZERO,
        }
    }
}

impl UnitShade {
    /// The mix as its parts' tags carry it.
    pub(crate) fn byte(&self) -> u8 {
        (self.t * 255.0).round().clamp(0.0, 255.0) as u8
    }

    /// The client's intensity for the mix: 2.5 lit, 0.5 in shadow.
    pub(crate) fn intensity(&self) -> f32 {
        2.5 - 2.0 * self.t
    }

    fn effective_target(&self) -> f32 {
        if self.indoor {
            INDOOR_T
        } else if self.on_wmo {
            LIT_T
        } else {
            self.target
        }
    }

    pub(crate) fn ramps_settled(&self) -> bool {
        (self.t - self.effective_target()).abs() < SETTLED_EPS
            && (self.ambient - self.ambient_target).abs().max_element() < SETTLED_EPS
    }

    /// Entering a room, the ambient ramps from the scene's toward the room's.
    pub(crate) fn seed_ambient(&mut self, from: Vec3, target: Vec3) {
        self.ambient = from;
        self.ambient_target = target;
    }
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

/// A unit lit by a probe of its own holds the probe's slot where the shade byte would go.
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
        let byte = u32::from(shade.byte());
        let cards = cards.map_or(&[][..], |c| &c.0[..]);
        for part in children.iter_descendants(root).chain(cards.iter().copied()) {
            if let Ok(mut tag) = tags.get_mut(part)
                && (tag.0 & SHADE_MASK) >> SHADE_SHIFT != byte
            {
                tag.0 = (tag.0 & !SHADE_MASK) | (byte << SHADE_SHIFT);
            }
        }
    }
}
