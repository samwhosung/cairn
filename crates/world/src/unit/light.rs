use bevy::prelude::*;

use super::body::{BodyDressed, BodyModel};
use super::shade::UnitShade;
use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::ground::terrain_wow_z_under;
use crate::interior::{FEET_PROBE_LIFT, FaceBelow, WmoGeneration, WmoRoom, face_below};
use crate::light::SceneLight;
use crate::m2::M2Model;
use crate::portal::{EXTERIOR, EXTERIOR_LIT, WmoPortalInstance, terrain_z_local};
use crate::probes::{ProbeSlot, Probes, PropLobeLight, fold_interior_probe};
use crate::stream::Streamer;
use crate::surface::footprint_under;
use crate::wmo::{WmoModel, cap96, unit_floor_diffuse};

const STILL_DIST_SQ: f32 = 1.0e-4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LitBy {
    Sky,
    SkyIndoors,
    OwnProbe { slot: u16 },
}

#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct UnitLight {
    pub(crate) lit_by: LitBy,
    pub(crate) room_fogged: bool,
}

impl UnitLight {
    pub(crate) fn probe_lit(self) -> bool {
        matches!(self.lit_by, LitBy::OwnProbe { .. })
    }
}

#[derive(Component)]
pub(crate) struct LightRay {
    room: Option<WmoRoom>,
    cast_from: Vec3,
    wmo_generation: u32,
}

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
    face: FaceBelow,
    model: &'a WmoModel,
    instance: &'a WmoPortalInstance,
    entity: Entity,
    local: [f32; 3],
}

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
        let Some(face) = face_below(model, local, terrain_local, EXTERIOR | EXTERIOR_LIT) else {
            continue;
        };
        if best.as_ref().is_none_or(|b| face.depth < b.face.depth) {
            best = Some(Winner {
                face,
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
    if w.face.outdoor_by_mask {
        return (Verdict::OnBuildingOutdoors, None);
    }
    let room = Some(WmoRoom {
        instance: w.entity,
        group: u16::try_from(w.face.group).unwrap_or(u16::MAX),
    });
    let Some(hit) = footprint_under(w.model, w.local, None).filter(|h| !h.lit_by_day_night) else {
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
            mocv: hit.mocv_at_hit,
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

fn is_room_fogged(
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
        Option<&'static ProbeSlot>,
    ),
    With<BodyDressed>,
>;

#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_unit_light(
    mut commands: Commands<'_, '_>,
    wmos: Res<'_, Assets<WmoModel>>,
    m2s: Res<'_, Assets<M2Model>>,
    instances: Query<'_, '_, (Entity, &WmoPortalInstance)>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    generation: Res<'_, WmoGeneration>,
    scene: Option<Res<'_, SceneLight>>,
    mut probes: ResMut<'_, Probes>,
    mut units: Units<'_, '_>,
) {
    for (entity, gt, model, mut shade, mut light, ray, probe_fold, held_slot) in &mut units {
        let pos = gt.translation();
        let still = ray.as_ref().is_some_and(|r| {
            r.wmo_generation == generation.0 && pos.distance_squared(r.cast_from) < STILL_DIST_SQ
        });
        if still && let (Some(light), Some(ray)) = (light.as_mut(), ray.as_ref()) {
            if let (LitBy::OwnProbe { slot }, Some(f)) = (light.lit_by, probe_fold)
                && !shade.ramps_settled()
            {
                probes.update_owned(slot, fold(&shade, f));
            }
            let fogged = is_room_fogged(ray.room, &instances);
            if light.room_fogged != fogged {
                light.room_fogged = fogged;
            }
            continue;
        }
        let held_slot = held_slot.map(|s| s.0);
        let (verdict, room) = light_verdict_at(&wmos, instances.iter(), &ground.0, &ground.1, pos);
        shade.on_building_outdoors = matches!(verdict, Verdict::OnBuildingOutdoors);
        let lit_by = match verdict {
            Verdict::Outdoors | Verdict::OnBuildingOutdoors => LitBy::Sky,
            Verdict::DayNight => LitBy::SkyIndoors,
            Verdict::Baked { mocv, lobes } => {
                let center = m2s
                    .get(&model.0)
                    .map_or(Vec3::ZERO, |m| m.fade_sphere(1.0).1);
                let target = Vec3::from_array(cap96(mocv));
                if held_slot.is_none() {
                    let from = scene
                        .as_ref()
                        .map_or(target, |s| Vec3::from_array(s.ambient));
                    shade.seed_ambient(from, target);
                } else {
                    shade.ambient_target = target;
                }
                let f = ProbeFold {
                    word: Vec3::from_array(unit_floor_diffuse(mocv)),
                    lobes,
                    ref_point: gt.transform_point(center),
                };
                let coeffs = fold(&shade, &f);
                let slot = match held_slot {
                    Some(slot) => {
                        probes.update_owned(slot, coeffs);
                        Some(slot)
                    }
                    None => probes.alloc_owned(coeffs),
                };
                if let Some(slot) = slot {
                    commands.entity(entity).try_insert(f);
                    LitBy::OwnProbe { slot }
                } else {
                    warn_once!("the probe table is full: a unit indoors is lit by the sky");
                    LitBy::SkyIndoors
                }
            }
        };
        shade.indoor = lit_by != LitBy::Sky;
        match (held_slot, lit_by) {
            (Some(old), LitBy::OwnProbe { slot: new }) if old == new => {}
            (_, LitBy::OwnProbe { slot: new }) => {
                commands.entity(entity).try_insert(ProbeSlot(new));
            }
            (Some(_), _) => {
                commands
                    .entity(entity)
                    .try_remove::<(ProbeSlot, ProbeFold)>();
            }
            (None, _) => {}
        }
        let room = room.filter(|_| lit_by != LitBy::Sky);
        let next = UnitLight {
            lit_by,
            room_fogged: is_room_fogged(room, &instances),
        };
        let next_ray = LightRay {
            room,
            cast_from: pos,
            wmo_generation: generation.0,
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
