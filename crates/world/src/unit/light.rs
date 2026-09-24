//! A unit's light in a building. A ray down from its feet finds the face the client lights it by:
//! a building's outdoor surface keeps it on the sky, an indoor floor's baked colour and its room's
//! lamps fold into a probe of its own, and a floor that asks for the day and night gives it the
//! sky at the room's intensity. Indoors the room's fog is its fog while the camera's rooms reach
//! the room.

use bevy::prelude::*;

use super::body::{BodyDressed, BodyModel};
use super::shade::UnitShade;
use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::ground::terrain_wow_z_under;
use crate::interior::{DownRayClaim, FEET_PROBE_LIFT, WmoGeneration, WmoRoom, down_ray_claim};
use crate::light::SceneLight;
use crate::m2::M2Model;
use crate::portal::{EXTERIOR, EXTERIOR_LIT, WmoPortalInstance, terrain_z_local};
use crate::probes::{PropLobeLight, PropProbeSlot, PropProbes, fold_interior_probe};
use crate::stream::Streamer;
use crate::surface::footprint_under;
use crate::wmo::{WmoModel, cap96, floor168};

/// A unit that has not moved further than this since its last ray keeps its verdict; the client
/// re-rays a unit every frame, so this only spares the still ones.
const RESAMPLE_DIST_SQ: f32 = 1.0e-4;

/// How the client lights a unit, from where it stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Law {
    /// The sky, and the ground's baked shadow under it.
    Exterior,
    /// Indoors on the sky at the room's intensity: the floor asks for it, or no floor was found.
    DayNight,
    /// Indoors on its own probe, this slot, folded from the floor's colour and the room's lamps.
    Probe(u16),
}

/// The law a unit's parts are drawn by, and whether its room's fog is its fog: the room is on the
/// camera's rooms.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnitLight {
    pub(crate) law: Law,
    pub(crate) fog: bool,
}

impl UnitLight {
    pub(crate) fn probe_lit(self) -> bool {
        matches!(self.law, Law::Probe(_))
    }
}

/// Where a unit's last ray was cast from, when, and the room it found.
#[derive(Component)]
pub(crate) struct LightRay {
    room: Option<WmoRoom>,
    at: Vec3,
    generation: u32,
}

/// What a probe-lit unit's probe is folded from, kept so the probe follows the unit's ramps
/// without another ray while it stands still.
#[derive(Component)]
pub(crate) struct ProbeFold {
    word: Vec3,
    lobes: Vec<PropLobeLight>,
    ref_point: Vec3,
}

enum Verdict {
    Outdoors,
    OnBuildingOutdoors,
    DayNight,
    Baked {
        mocv: [u8; 3],
        lobes: Vec<PropLobeLight>,
    },
}

struct Winner<'a> {
    claim: DownRayClaim,
    model: &'a WmoModel,
    instance: &'a WmoPortalInstance,
    entity: Entity,
    local: [f32; 3],
}

/// Every placement's nearest face under the feet races the terrain under it and the other
/// placements' faces, the nearest winning; an indoor winner's render faces give the colour.
fn light_verdict_at<'a>(
    wmos: &'a Assets<WmoModel>,
    instances: impl Iterator<Item = (Entity, &'a WmoPortalInstance)>,
    streamer: &Streamer,
    adts: &Assets<AdtTile>,
    feet: Vec3,
) -> (Verdict, Option<WmoRoom>) {
    let probe = feet + Vec3::Y * FEET_PROBE_LIFT;
    let terrain = terrain_wow_z_under(streamer, adts, probe);
    let mut best: Option<Winner<'a>> = None;
    for (entity, instance) in instances {
        let Some(model) = wmos.get(&instance.handle) else {
            continue;
        };
        let local_from_world = instance.world_from_local.inverse();
        let local = bevy_to_wow(local_from_world.transform_point3(probe));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
        let Some(claim) = down_ray_claim(model, local, terrain_local, EXTERIOR | EXTERIOR_LIT)
        else {
            continue;
        };
        if best.as_ref().is_none_or(|b| claim.depth < b.claim.depth) {
            best = Some(Winner {
                claim,
                model,
                instance,
                entity,
                local,
            });
        }
    }
    let Some(w) = best else {
        return (Verdict::Outdoors, None);
    };
    if w.claim.outdoor {
        return (Verdict::OnBuildingOutdoors, None);
    }
    let room = Some(WmoRoom {
        instance: w.entity,
        group: u16::try_from(w.claim.group).unwrap_or(u16::MAX),
    });
    let Some(hit) = footprint_under(w.model, w.local, None).filter(|h| !h.day_night) else {
        return (Verdict::DayNight, room);
    };
    let lobes = w
        .model
        .group_light_refs
        .get(hit.group)
        .into_iter()
        .flatten()
        .filter_map(|&li| w.model.lights.get(usize::from(li)))
        .filter(|l| l.is_omni())
        .map(|l| PropLobeLight {
            pos: w
                .instance
                .world_from_local
                .transform_point3(wow_to_bevy(l.position)),
            color_i: l.color.map(|c| c * l.intensity.max(0.0)),
            atten_start: l.attenuation_start,
            atten_end: l.attenuation_end,
        })
        .collect();
    (
        Verdict::Baked {
            mocv: hit.mocv,
            lobes,
        },
        room,
    )
}

fn fold(shade: &UnitShade, fold: &ProbeFold) -> [Vec4; 7] {
    fold_interior_probe(
        shade.ambient.to_array(),
        (fold.word * shade.intensity()).to_array(),
        fold.ref_point,
        &fold.lobes,
    )
}

fn room_fogged(
    room: Option<WmoRoom>,
    instances: &Query<'_, '_, (Entity, &WmoPortalInstance)>,
) -> bool {
    room.and_then(|r| {
        let (_, inst) = instances.get(r.instance).ok()?;
        inst.interior_fog.get(usize::from(r.group)).copied()
    })
    .unwrap_or(false)
}

type Units<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static BodyModel,
        &'static mut UnitShade,
        Option<&'static mut UnitLight>,
        Option<&'static mut LightRay>,
        Option<&'static ProbeFold>,
        Option<&'static PropProbeSlot>,
    ),
    With<BodyDressed>,
>;

/// A unit entering a room ramps its ambient from the scene's toward the room's, and keeps its
/// probe's slot for as long as it stays probe-lit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_unit_light(
    mut commands: Commands<'_, '_>,
    wmos: Res<'_, Assets<WmoModel>>,
    m2s: Res<'_, Assets<M2Model>>,
    instances: Query<'_, '_, (Entity, &WmoPortalInstance)>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    generation: Res<'_, WmoGeneration>,
    scene: Option<Res<'_, SceneLight>>,
    mut probes: ResMut<'_, PropProbes>,
    mut units: Units<'_, '_>,
) {
    for (entity, gt, model, mut shade, mut light, ray, probe_fold, seated) in &mut units {
        let pos = gt.translation();
        let still = ray.as_ref().is_some_and(|r| {
            r.generation == generation.0 && pos.distance_squared(r.at) < RESAMPLE_DIST_SQ
        });
        if still && let (Some(light), Some(ray)) = (light.as_mut(), ray.as_ref()) {
            if let (Law::Probe(slot), Some(f)) = (light.law, probe_fold)
                && !shade.ramps_settled()
            {
                probes.update_owned(slot, fold(&shade, f));
            }
            let fog = room_fogged(ray.room, &instances);
            if light.fog != fog {
                light.fog = fog;
            }
            continue;
        }
        let seated = seated.map(|s| s.0);
        let (verdict, room) = light_verdict_at(&wmos, instances.iter(), &ground.0, &ground.1, pos);
        shade.on_wmo = matches!(verdict, Verdict::OnBuildingOutdoors);
        let law = match verdict {
            Verdict::Outdoors | Verdict::OnBuildingOutdoors => Law::Exterior,
            Verdict::DayNight => Law::DayNight,
            Verdict::Baked { mocv, lobes } => {
                let center = m2s
                    .get(&model.0)
                    .map_or(Vec3::ZERO, |m| m.fade_sphere(1.0).1);
                let target = Vec3::from_array(cap96(mocv));
                if seated.is_none() {
                    let from = scene
                        .as_ref()
                        .map_or(target, |s| Vec3::from_array(s.ambient));
                    shade.seed_ambient(from, target);
                } else {
                    shade.ambient_target = target;
                }
                let f = ProbeFold {
                    word: Vec3::from_array(floor168(mocv)),
                    lobes,
                    ref_point: gt.transform_point(center),
                };
                let coeffs = fold(&shade, &f);
                let slot = match seated {
                    Some(slot) => {
                        probes.update_owned(slot, coeffs);
                        Some(slot)
                    }
                    None => probes.alloc_owned(coeffs),
                };
                if let Some(slot) = slot {
                    commands.entity(entity).try_insert(f);
                    Law::Probe(slot)
                } else {
                    warn_once!("the probe table is full: a unit indoors is lit by the sky");
                    Law::DayNight
                }
            }
        };
        shade.indoor = law != Law::Exterior;
        match (seated, law) {
            (Some(old), Law::Probe(new)) if old == new => {}
            (_, Law::Probe(new)) => {
                commands.entity(entity).try_insert(PropProbeSlot(new));
            }
            (Some(_), _) => {
                commands
                    .entity(entity)
                    .try_remove::<(PropProbeSlot, ProbeFold)>();
            }
            (None, _) => {}
        }
        let room = room.filter(|_| law != Law::Exterior);
        let next = UnitLight {
            law,
            fog: room_fogged(room, &instances),
        };
        let next_ray = LightRay {
            room,
            at: pos,
            generation: generation.0,
        };
        match (light, ray) {
            (Some(mut light), Some(mut ray)) => {
                light.set_if_neq(next);
                *ray = next_ray;
            }
            _ => {
                commands.entity(entity).try_insert((next, next_ray));
            }
        }
    }
}
