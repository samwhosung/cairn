use bevy::mesh::MeshTag;
use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::model_material::GroundShade;
use crate::models::ground_shade;
use crate::stream::Streamer;

/// Where the mix sits between the lit and the shadowed intensity: 0 is the client's lit 2.5, 1
/// its shadowed 0.5. The shader caps the intensity at 1, so the lit end is aimed at the mix that
/// shows as 1, 0.75.
const LIT_T: f32 = 0.75;
const SHADOWED_T: f32 = 1.0;
/// The client ramps the intensity at 3.3333 a second over the 2.0-wide span.
const RAMP_PER_SEC: f32 = 3.3333 / 2.0;
const RESAMPLE_DIST: f32 = 0.5;

const SHADE_MASK: u32 = 0x0000_3fc0;
const SHADE_SHIFT: u32 = 6;

/// A unit's own light: the baked ground shadow under its feet, ramped as it walks in and out of
/// it, carried to every part it draws.
#[derive(Component)]
pub struct UnitShade {
    t: f32,
    target: f32,
    last_pos: Vec3,
    sampled: bool,
}

impl Default for UnitShade {
    fn default() -> Self {
        Self {
            t: LIT_T,
            target: LIT_T,
            last_pos: Vec3::ZERO,
            sampled: false,
        }
    }
}

pub(crate) fn update_unit_shade(
    time: Res<'_, Time>,
    streamer: Res<'_, Streamer>,
    adts: Res<'_, Assets<AdtTile>>,
    mut units: Query<'_, '_, (Entity, &GlobalTransform, &mut UnitShade)>,
    children: Query<'_, '_, &Children>,
    mut tags: Query<'_, '_, &mut MeshTag>,
) {
    let step = RAMP_PER_SEC * time.delta_secs();
    for (root, gt, mut shade) in &mut units {
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
                    shade.t = shade.target;
                    shade.sampled = true;
                }
            }
        }
        let target = shade.target;
        shade.t = if shade.t < target {
            (shade.t + step).min(target)
        } else {
            (shade.t - step).max(target)
        };
        let byte = u32::from((shade.t * 255.0).round().clamp(0.0, 255.0) as u8);
        for part in children.iter_descendants(root) {
            if let Ok(mut tag) = tags.get_mut(part)
                && (tag.0 & SHADE_MASK) >> SHADE_SHIFT != byte
            {
                tag.0 = (tag.0 & !SHADE_MASK) | (byte << SHADE_SHIFT);
            }
        }
    }
}
