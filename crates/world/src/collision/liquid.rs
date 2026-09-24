//! Where the liquid is for the body and the camera boom: every streamed liquid surface's grid,
//! the building a body stands in, and the waterline the boom may hit. Inside a building only its
//! own liquid answers, so a canal does not claim the tunnel under it.

use avian3d::prelude::{Collider, RigidBody};
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;
use terrain::LiquidMesh;

use super::assets::{TileCollision, WmoHull};
use super::stream::CollisionStreamer;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::interior::WmoRoom;
use crate::liquid::{
    LiquidClaim, LiquidGrid, LiquidHit, SpatialIndex, liquid_at, water_surface_at,
};
use crate::portal::{down_ray_seeds, terrain_z_local};

/// How far over the feet the down-ray that finds a body's room starts, yards.
const ROOM_PROBE_HEIGHT: f32 = 1.7;

/// One liquid surface the body swims in and the camera boom meets.
#[derive(Component)]
pub struct LiquidSurface(pub(crate) LiquidGrid);

/// A placed building's rooms: whoever stands in one of them reads that building's liquid.
#[derive(Component)]
pub(super) struct PlacedRooms {
    pub(super) hull: Handle<WmoHull>,
    pub(super) world_from_local: Affine3A,
}

#[derive(Resource, Default)]
pub(crate) struct WaterIndex(SpatialIndex);

pub(super) fn maintain_water_index(
    mut index: ResMut<'_, WaterIndex>,
    added: Query<'_, '_, (), Added<LiquidSurface>>,
    mut removed: RemovedComponents<'_, '_, LiquidSurface>,
    surfaces: Query<'_, '_, (Entity, &LiquidSurface)>,
) {
    if removed.read().next().is_none() && added.is_empty() {
        return;
    }
    index.0.rebuild(surfaces.iter().map(|(e, s)| (e, &s.0)));
}

/// The waterline the camera may hit: the surface's wet triangles, carried into the world by
/// `transform`. `None` when none is wet.
pub(super) fn liquid_collider(
    mesh: &LiquidMesh,
    transform: &Transform,
) -> Option<(Collider, RigidBody)> {
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    if tris.is_empty() {
        return None;
    }
    let verts = mesh
        .positions
        .iter()
        .map(|p| transform.transform_point(wow_to_bevy(*p)))
        .collect();
    Some((Collider::trimesh(verts, tris), RigidBody::Static))
}

/// The nearest point of one sound class's wet footprints, by their boxes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NearestLiquid {
    pub dist_sq: f32,
    /// WoW space.
    pub point: [f32; 3],
    pub nibble: u8,
}

/// The liquid under a point, from the streamed surfaces.
#[derive(SystemParam)]
pub struct Liquids<'w, 's> {
    index: Res<'w, WaterIndex>,
    surfaces: Query<'w, 's, &'static LiquidSurface>,
    rooms: Query<'w, 's, (Entity, &'static PlacedRooms)>,
    hulls: Res<'w, Assets<WmoHull>>,
    tiles: Res<'w, Assets<TileCollision>>,
    streamer: Res<'w, CollisionStreamer>,
}

impl Liquids<'_, '_> {
    /// The liquid over a body's feet, at a WoW position: the building's own when the body stands
    /// in one, the open world's otherwise. Where surfaces stack, the lowest.
    pub fn liquid_at(&self, feet: [f32; 3]) -> Option<LiquidHit> {
        let near = self
            .index
            .0
            .over(feet[0], feet[1])
            .iter()
            .filter_map(|&e| self.surfaces.get(e).ok())
            .map(|s| &s.0);
        liquid_at(near, feet, self.claim_at(feet))
    }

    /// The water surface over a body's feet, lava and slime left out; where waters overlap, the
    /// lowest.
    pub fn water_surface_at(&self, feet: [f32; 3]) -> Option<f32> {
        let near = self
            .index
            .0
            .over(feet[0], feet[1])
            .iter()
            .filter_map(|&e| self.surfaces.get(e).ok())
            .map(|s| &s.0);
        water_surface_at(near, feet, self.claim_at(feet))
    }

    /// The nearest point of each sound class's wet footprints, by their boxes, within `radius` of
    /// a WoW position: one per class `nibble & 3`.
    pub fn nearest_per_class(&self, wow: [f32; 3], radius: f32) -> [Option<NearestLiquid>; 4] {
        let mut best: [Option<NearestLiquid>; 4] = [None; 4];
        for LiquidSurface(grid) in &self.surfaces {
            let Some(point) = grid.nearest_point(wow[0], wow[1]) else {
                continue;
            };
            let dist_sq = (point[0] - wow[0]).powi(2)
                + (point[1] - wow[1]).powi(2)
                + (point[2] - wow[2]).powi(2);
            let nibble = grid.sound_nibble();
            let class = usize::from(nibble & 3);
            if dist_sq <= radius * radius && best[class].is_none_or(|b| dist_sq < b.dist_sq) {
                best[class] = Some(NearestLiquid {
                    dist_sq,
                    point,
                    nibble,
                });
            }
        }
        best
    }

    /// The first building whose room down-ray, cast from a little over the feet, finds an
    /// indoor group under them; the open world when none does.
    fn claim_at(&self, feet: [f32; 3]) -> LiquidClaim {
        let probe = wow_to_bevy(feet) + Vec3::Y * ROOM_PROBE_HEIGHT;
        let terrain = self.streamer.terrain_z_under(&self.tiles, probe);
        for (instance, placed) in &self.rooms {
            let Some(hull) = self.hulls.get(&placed.hull) else {
                continue;
            };
            if hull.rooms.wmo_id == 0 {
                continue;
            }
            let local_from_world = placed.world_from_local.inverse();
            let eye = bevy_to_wow(local_from_world.transform_point3(probe));
            let ground = terrain.map(|z| terrain_z_local(&local_from_world, probe, z));
            if let Some(group) = down_ray_seeds(&hull.rooms, eye, ground).in_group {
                let room = WmoRoom {
                    instance,
                    group: group as u16,
                };
                return LiquidClaim::inside(room, &hull.rooms.group_nav);
            }
        }
        LiquidClaim::Outdoors
    }
}
