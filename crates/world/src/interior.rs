//! Which building the player stands in, what the world calls the place, and which room each body
//! stands in. Every answer is a vertical ray down from a point racing the terrain under it: the
//! nearer surface wins the column, a tie keeps the building.

use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::ground::{Ground, ground_under, terrain_wow_z_under};
use crate::portal::{WmoPortalInstance, down_ray_seeds, terrain_z_local};
use crate::stream::Streamer;
use crate::unit::UnitBody;
use crate::view::WorldCamera;
use crate::wmo::{Bounds, Triangle, WmoGroupNav, WmoModel};
use crate::wmo_areas::WmoAreas;

/// The group flag of a building's outdoor groups.
const EXTERIOR: u32 = 0x8;
/// The render and sound claim casts from the chest, over the body's feet.
const INTERIOR_PROBE_HEIGHT: f32 = 1.7;
/// The position claims cast from just over the feet, so a floor they rest on is not lost to a
/// rounding hair.
pub(crate) const POSITION_PROBE_LIFT: f32 = 0.1;
/// How far down the position claims reach.
const ZONE_RAY_LEN: f32 = 1000.0;
/// A body whose origin sits below its own floor finds nothing under it, so a miss casts again
/// from this far over its feet: over the middle of any playable body.
const ROOM_UNDER_FLOOR_TOLERANCE: f32 = 2.0;
/// How far a body moves before its room is cast again.
const UNIT_ROOM_RESAMPLE_DIST_SQ: f32 = 0.25 * 0.25;

/// The player's body, when the eye is on one: its feet in Bevy space, and whether the world under
/// it has arrived. The app writes it.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct Viewer {
    pub body: Option<Vec3>,
    pub settled: bool,
}

/// `WMOAreaTable`'s keys for a group of a placed building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WmoInteriorKeys {
    pub wmo_id: u32,
    pub name_set: u32,
    pub group_area_id: u32,
}

/// A group of one placed building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WmoRoom {
    pub instance: Entity,
    pub group: u16,
}

/// The building group the render and the sound place the player in, from the chest down through
/// faces and portals; `None` outdoors.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentWmoInterior(pub Option<WmoInteriorKeys>);

/// The same claim as the placement and group it names.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerWmoRoom(pub Option<WmoRoom>);

/// Where the area's name comes from indoors: from the feet down through faces alone, so a
/// doorway's portal under the eye does not make the player indoors.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentAreaInterior(pub Option<WmoInteriorKeys>);

/// The `AreaTable` id the player stands in: the building's own area indoors, else the terrain
/// chunk's. Held through a tile still decoding, gone with the body.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentArea(pub Option<u32>);

/// The room a body stands in, cast from its own position; re-cast as it moves or as buildings
/// come and go.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
pub struct UnitRoom {
    room: Option<WmoRoom>,
    at: Vec3,
    generation: u32,
}

impl UnitRoom {
    pub fn room(&self) -> Option<WmoRoom> {
        self.room
    }
}

/// Counts the placed buildings arriving and leaving, so a standing body re-casts its room.
#[derive(Resource, Default)]
pub(crate) struct WmoGeneration(u32);

pub(crate) fn count_buildings(
    mut generation: ResMut<'_, WmoGeneration>,
    added: Query<'_, '_, (), Added<WmoPortalInstance>>,
    mut removed: RemovedComponents<'_, '_, WmoPortalInstance>,
) {
    if !added.is_empty() || removed.read().next().is_some() {
        generation.0 = generation.0.wrapping_add(1);
    }
}

fn eye_or_camera(
    viewer: &Viewer,
    camera: &Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    lift: f32,
) -> Option<Vec3> {
    match viewer.body {
        Some(feet) => Some(feet + Vec3::Y * lift),
        None => camera.iter().next().map(GlobalTransform::translation),
    }
}

fn keys(model: &WmoModel, inst: &WmoPortalInstance, group: usize) -> WmoInteriorKeys {
    WmoInteriorKeys {
        wmo_id: model.wmo_id,
        name_set: u32::from(inst.name_set),
        group_area_id: model.group_nav.get(group).map_or(0, |g| g.area_table_id),
    }
}

/// The render and sound claim: the first placed building whose down-ray names a group.
pub(crate) fn track_current_interior(
    wmos: Res<'_, Assets<WmoModel>>,
    viewer: Res<'_, Viewer>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    instances: Query<'_, '_, (Entity, &WmoPortalInstance)>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    mut current: ResMut<'_, CurrentWmoInterior>,
    mut room: ResMut<'_, PlayerWmoRoom>,
) {
    let Some(eye) = eye_or_camera(&viewer, &camera, INTERIOR_PROBE_HEIGHT) else {
        return;
    };
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, eye);
    let mut found = (None, None);
    for (entity, inst) in &instances {
        let Some(model) = wmos.get(&inst.handle).filter(|m| m.wmo_id != 0) else {
            continue;
        };
        let local_from_world = inst.world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye, z));
        if let Some(gi) = down_ray_seeds(model, eye_local, terrain_local).in_group {
            let group = gi as u16;
            found = (
                Some(keys(model, inst, gi)),
                Some(WmoRoom {
                    instance: entity,
                    group,
                }),
            );
            break;
        }
    }
    current.set_if_neq(CurrentWmoInterior(found.0));
    room.set_if_neq(PlayerWmoRoom(found.1));
}

/// The area's indoor claim: faces alone, outdoor groups excluded.
pub(crate) fn track_area_interior(
    wmos: Res<'_, Assets<WmoModel>>,
    viewer: Res<'_, Viewer>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    mut current: ResMut<'_, CurrentAreaInterior>,
) {
    let Some(probe) = eye_or_camera(&viewer, &camera, POSITION_PROBE_LIFT) else {
        return;
    };
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, probe);
    let mut found = None;
    for inst in &instances {
        let Some(model) = wmos.get(&inst.handle).filter(|m| m.wmo_id != 0) else {
            continue;
        };
        let local_from_world = inst.world_from_local.inverse();
        let local = bevy_to_wow(local_from_world.transform_point3(probe));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
        if let Some(gi) = area_down_ray(model, local, terrain_local, EXTERIOR) {
            found = Some(keys(model, inst, gi));
            break;
        }
    }
    current.set_if_neq(CurrentAreaInterior(found));
}

/// The building group's own area when the area claim names one that has it, else the chunk's.
pub(crate) fn update_current_area(
    viewer: Res<'_, Viewer>,
    interior: Res<'_, CurrentAreaInterior>,
    areas: Option<Res<'_, WmoAreas>>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    mut area: ResMut<'_, CurrentArea>,
) {
    let Some(feet) = viewer.body else {
        area.set_if_neq(CurrentArea(None));
        return;
    };
    if !viewer.settled {
        return;
    }
    let indoors = interior.0.zip(areas.as_deref()).and_then(|(k, areas)| {
        areas
            .resolve(k.wmo_id, k.name_set, k.group_area_id)
            .map(|a| a.area_table_id)
            .filter(|&id| id != 0)
    });
    let found = indoors.or_else(|| match ground_under(&ground.0, &ground.1, feet) {
        Ground::Tile(adt) => terrain::area_id_at(&adt.chunks, bevy_to_wow(feet)),
        _ => None,
    });
    if let Some(id) = found.filter(|&id| id != 0) {
        area.set_if_neq(CurrentArea(Some(id)));
    }
}

/// Keeps every body's [`UnitRoom`], from its own feet.
pub(crate) fn track_unit_rooms(
    mut commands: Commands<'_, '_>,
    wmos: Res<'_, Assets<WmoModel>>,
    instances: Query<'_, '_, (Entity, &WmoPortalInstance)>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    generation: Res<'_, WmoGeneration>,
    mut bodies: Query<'_, '_, (Entity, &GlobalTransform, Option<&mut UnitRoom>), With<UnitBody>>,
) {
    for (entity, transform, claim) in &mut bodies {
        let pos = transform.translation();
        if let Some(claim) = claim.as_ref()
            && claim.generation == generation.0
            && pos.distance_squared(claim.at) < UNIT_ROOM_RESAMPLE_DIST_SQ
        {
            continue;
        }
        let cast = |rise: f32| room_cast(&wmos, &instances, &ground, pos, rise);
        let next = UnitRoom {
            room: cast(0.0).or_else(|| cast(ROOM_UNDER_FLOOR_TOLERANCE)),
            at: pos,
            generation: generation.0,
        };
        match claim {
            Some(mut claim) => {
                claim.set_if_neq(next);
            }
            None => {
                commands.entity(entity).try_insert(next);
            }
        }
    }
}

fn room_cast(
    wmos: &Assets<WmoModel>,
    instances: &Query<'_, '_, (Entity, &WmoPortalInstance)>,
    ground: &(Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    feet: Vec3,
    rise: f32,
) -> Option<WmoRoom> {
    let probe = feet + Vec3::Y * (POSITION_PROBE_LIFT + rise);
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, probe);
    instances.iter().find_map(|(entity, inst)| {
        let model = wmos.get(&inst.handle).filter(|m| m.wmo_id != 0)?;
        let local_from_world = inst.world_from_local.inverse();
        let local = bevy_to_wow(local_from_world.transform_point3(probe));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
        area_down_ray(model, local, terrain_local, EXTERIOR).map(|gi| WmoRoom {
            instance: entity,
            group: gi as u16,
        })
    })
}

/// The group owning the nearest collision face under `eye` (model space) within the ray, unless
/// strictly nearer terrain takes the column or the group carries a flag of `outdoor_mask`.
pub(crate) fn area_down_ray(
    model: &WmoModel,
    eye: [f32; 3],
    terrain_z: Option<f32>,
    outdoor_mask: u32,
) -> Option<usize> {
    let (group, best_z) = nearest_face_below(
        &model.group_collision_tris,
        &model.group_collision_bounds,
        eye,
    )?;
    if eye[2] - best_z > ZONE_RAY_LEN || terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return None;
    }
    let outdoor = model
        .group_nav
        .get(group)
        .is_none_or(|g: &WmoGroupNav| g.flags & outdoor_mask != 0);
    (!outdoor).then_some(group)
}

/// Faces are culled by their own boxes, never a group's authored box, which can float above its
/// floor; an exact tie keeps the first face found.
fn nearest_face_below(
    tris: &[Vec<Triangle>],
    bounds: &[Option<Bounds>],
    eye: [f32; 3],
) -> Option<(usize, f32)> {
    let mut best: Option<(usize, f32)> = None;
    for (gi, group) in tris.iter().enumerate() {
        let owns_column = bounds.get(gi).copied().flatten().is_some_and(|(min, max)| {
            (min[0]..=max[0]).contains(&eye[0])
                && (min[1]..=max[1]).contains(&eye[1])
                && min[2] <= eye[2]
        });
        if !owns_column {
            continue;
        }
        for tri in group {
            if let Some(z) = terrain::triangle_z_at(tri, eye[0], eye[1])
                && z <= eye[2]
                && best.is_none_or(|(_, bz)| z > bz)
            {
                best = Some((gi, z));
            }
        }
    }
    best
}
