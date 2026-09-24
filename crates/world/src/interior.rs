//! Which building the player stands in, what the world calls the place, and which room each body
//! stands in. Every answer is a vertical ray down from a point racing the terrain under it: the
//! nearer surface wins the column, a tie keeps the building.

use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::ground::{Ground, ground_under, terrain_wow_z_under};
use crate::portal::{EXTERIOR, WmoPortalInstance, down_ray_seeds, terrain_z_local};
use crate::stream::Streamer;
use crate::unit::UnitBody;
use crate::view::WorldCamera;
use crate::wmo::{Bounds, Triangle, WmoGroupNav, WmoModel};
use crate::wmo_areas::WmoAreas;

const CHEST_HEIGHT: f32 = 1.7;
pub(crate) const FEET_PROBE_LIFT: f32 = 0.1;
const FEET_RAY_REACH: f32 = 1000.0;
const SUNK_ORIGIN_RECAST_RISE: f32 = 2.0;
const UNIT_ROOM_RESAMPLE_DIST_SQ: f32 = 0.25 * 0.25;

/// The player's body, when the eye is on one: its feet in Bevy space, whether the world under it
/// has arrived, and how it moves. The app writes it.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct Viewer {
    pub body: Option<Vec3>,
    pub settled: bool,
    /// Moving forward, back or sideways.
    pub translating: bool,
    pub turning: bool,
    pub collision_height: f32,
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

/// The building group the sound places the player in, cast from the chest (from the camera when
/// there is no body) down through faces and portals; `None` outdoors.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentWmoInterior(pub Option<WmoInteriorKeys>);

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

#[derive(Resource, Default)]
pub(crate) struct WmoGeneration(pub(crate) u32);

pub(crate) fn bump_wmo_generation(
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
        wmo_id: model.rooms.wmo_id,
        name_set: u32::from(inst.name_set),
        group_area_id: model
            .rooms
            .group_nav
            .get(group)
            .map_or(0, |g| g.wmo_group_id),
    }
}

pub(crate) fn track_current_interior(
    wmos: Res<'_, Assets<WmoModel>>,
    viewer: Res<'_, Viewer>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    mut current: ResMut<'_, CurrentWmoInterior>,
) {
    let Some(eye) = eye_or_camera(&viewer, &camera, CHEST_HEIGHT) else {
        return;
    };
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, eye);
    let mut found = None;
    for inst in &instances {
        let Some(model) = wmos.get(&inst.handle).filter(|m| m.rooms.wmo_id != 0) else {
            continue;
        };
        let local_from_world = inst.world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye, z));
        if let Some(gi) = down_ray_seeds(&model.rooms, eye_local, terrain_local).in_group {
            found = Some(keys(model, inst, gi));
            break;
        }
    }
    current.set_if_neq(CurrentWmoInterior(found));
}

pub(crate) fn track_area_interior(
    wmos: Res<'_, Assets<WmoModel>>,
    viewer: Res<'_, Viewer>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    mut current: ResMut<'_, CurrentAreaInterior>,
) {
    let Some(probe) = eye_or_camera(&viewer, &camera, FEET_PROBE_LIFT) else {
        return;
    };
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, probe);
    let mut found = None;
    for inst in &instances {
        let Some(model) = wmos.get(&inst.handle).filter(|m| m.rooms.wmo_id != 0) else {
            continue;
        };
        let local_from_world = inst.world_from_local.inverse();
        let local = bevy_to_wow(local_from_world.transform_point3(probe));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
        if let Some(gi) = interior_group_under(model, local, terrain_local) {
            found = Some(keys(model, inst, gi));
            break;
        }
    }
    current.set_if_neq(CurrentAreaInterior(found));
}

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
            room: cast(0.0).or_else(|| cast(SUNK_ORIGIN_RECAST_RISE)),
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
    let probe = feet + Vec3::Y * (FEET_PROBE_LIFT + rise);
    let terrain = terrain_wow_z_under(&ground.0, &ground.1, probe);
    instances.iter().find_map(|(entity, inst)| {
        let model = wmos.get(&inst.handle).filter(|m| m.rooms.wmo_id != 0)?;
        let local_from_world = inst.world_from_local.inverse();
        let local = bevy_to_wow(local_from_world.transform_point3(probe));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
        interior_group_under(model, local, terrain_local).map(|gi| WmoRoom {
            instance: entity,
            group: gi as u16,
        })
    })
}

fn interior_group_under(
    model: &WmoModel,
    eye_model: [f32; 3],
    terrain_z: Option<f32>,
) -> Option<usize> {
    down_ray_claim(model, eye_model, terrain_z, EXTERIOR)
        .filter(|c| !c.outdoor)
        .map(|c| c.group)
}

/// The building face a ray down from a point meets first, if the building takes the column.
pub(crate) struct DownRayClaim {
    pub group: usize,
    /// How far below the point it lies; buildings are placed without scale, so depths compare
    /// across them.
    pub depth: f32,
    /// Its group carries one of the caller's outdoor flags.
    pub outdoor: bool,
}

pub(crate) fn down_ray_claim(
    model: &WmoModel,
    eye_model: [f32; 3],
    terrain_z: Option<f32>,
    outdoor_mask: u32,
) -> Option<DownRayClaim> {
    let (group, best_z) = nearest_face_below(
        &model.rooms.group_collision_tris,
        &model.rooms.group_collision_bounds,
        eye_model,
    )?;
    let depth = eye_model[2] - best_z;
    if depth > FEET_RAY_REACH || terrain_z.is_some_and(|tz| tz <= eye_model[2] && tz > best_z) {
        return None;
    }
    let outdoor = model
        .rooms
        .group_nav
        .get(group)
        .is_none_or(|g: &WmoGroupNav| g.flags & outdoor_mask != 0);
    Some(DownRayClaim {
        group,
        depth,
        outdoor,
    })
}

/// Groups are culled by the box of their own collision faces, never their authored box, which can
/// float above the floor.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portal::EXTERIOR_LIT;

    fn nav(flags: u32) -> WmoGroupNav {
        WmoGroupNav {
            flags,
            wmo_group_id: 0,
            bbox_min: [0.0; 3],
            bbox_max: [10.0; 3],
            ref_start: 0,
            ref_count: 0,
            interior: false,
            flooded: None,
            fog_indices: [0; 4],
        }
    }

    #[test]
    fn a_claim_is_the_nearest_face_below_under_the_callers_outdoor_flags() {
        let mut m = WmoModel::empty();
        let tri = |z: f32| [[0.0, 0.0, z], [10.0, 0.0, z], [0.0, 10.0, z]];
        m.rooms.group_collision_tris = vec![vec![tri(0.0)], vec![tri(2.0)]];
        m.rooms.group_collision_bounds = vec![
            Some(([0.0, 0.0, 0.0], [10.0, 10.0, 0.0])),
            Some(([0.0, 0.0, 2.0], [10.0, 10.0, 2.0])),
        ];
        m.rooms.group_nav = vec![nav(0), nav(EXTERIOR_LIT)];
        let eye = [1.0, 1.0, 5.0];
        let lit = down_ray_claim(&m, eye, None, EXTERIOR | EXTERIOR_LIT).expect("a face");
        assert_eq!((lit.group, lit.depth, lit.outdoor), (1, 3.0, true));
        let area = down_ray_claim(&m, eye, None, EXTERIOR).expect("a face");
        assert!(!area.outdoor, "a room lit as outdoors is still a room");
        assert!(down_ray_claim(&m, eye, Some(4.0), EXTERIOR).is_none());
        assert!(
            down_ray_claim(&m, eye, Some(2.0), EXTERIOR).is_some(),
            "a tie keeps it"
        );
    }
}
