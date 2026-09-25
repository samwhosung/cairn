//! Streams colliders with the terrain's window around the camera. A placement straddling tiles is
//! kept once, by unique id, while any of its tiles is.

use std::collections::{BTreeMap, BTreeSet};

use bevy::asset::LoadState;
use bevy::prelude::*;
use terrain::{Doodad, WmoInstance};

use super::assets::{M2Hull, TileCollision, WmoHull};
use super::colliders::{PendingCollider, build_collider_task, placement_collider_data};
use super::liquid::{PlacedRooms, SwimSurface, waterline_collider};
use super::weld::{Trimesh, spawn_batch, weld};
use super::{GroundDecalSurface, camera_layers, liquid_layers, walk_layers};
use crate::CurrentMap;
use crate::coords::{bevy_to_wow, placement_rotation, wmo_doodad_local, wow_to_bevy};
use crate::liquid::{LiquidSource, WmoPool, world_grid};
use crate::source::{MPQ_SOURCE, m2_url, wmo_url};
use crate::stream::Window;
use crate::view::{FARCLIP, WorldCamera};
use crate::wdt::WdtIndex;

/// Whether the collision around the camera has arrived: every tile of the window is built or
/// known to be missing, every placement's hull is in, and nothing waits to attach.
#[derive(Resource, Default, Debug)]
pub struct CollisionResidency {
    pending: usize,
    indexed: bool,
}

impl CollisionResidency {
    pub fn settled(&self) -> bool {
        self.indexed && self.pending == 0
    }

    /// Whether the map's index has been read, so that `pending` counts the whole window around
    /// the camera.
    pub fn indexed(&self) -> bool {
        self.indexed
    }

    /// Tiles, hulls, welds and colliders still on their way.
    pub fn pending(&self) -> usize {
        self.pending
    }
}

enum TileState {
    Unspawned(Handle<TileCollision>),
    Built(Handle<TileCollision>),
    Failed,
}

struct Tile {
    state: TileState,
    entities: Vec<Entity>,
    placements: BTreeSet<u32>,
    requested: u64,
}

impl Tile {
    fn in_flight(&self, tiles: &Assets<TileCollision>) -> bool {
        matches!(&self.state, TileState::Unspawned(handle) if tiles.get(handle).is_none())
    }
}

enum PlacementModel {
    M2 {
        hull: Handle<M2Hull>,
        welded: bool,
        inherited: bool,
    },
    Wmo {
        hull: Handle<WmoHull>,
        doodad_set: u16,
    },
}

struct Placement {
    model: Option<PlacementModel>,
    transform: Transform,
    refs: u32,
    owner: (u32, u32),
    entities: Vec<Entity>,
    unwelded_props: Vec<(Handle<M2Hull>, Transform)>,
}

#[derive(Resource, Default)]
pub(crate) struct CollisionStreamer {
    wdt: Option<Handle<WdtIndex>>,
    indexed: bool,
    tiles: BTreeMap<(u32, u32), Tile>,
    placements: BTreeMap<u32, Placement>,
    wanted: Vec<(u32, u32)>,
    requests: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn stream_collision(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    map: Res<'_, CurrentMap>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
    wdts: Res<'_, Assets<WdtIndex>>,
    tiles: Res<'_, Assets<TileCollision>>,
    mut streamer: ResMut<'_, CollisionStreamer>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let streamer = &mut *streamer;
    let dir = map.directory.to_ascii_lowercase();
    let wdt = streamer
        .wdt
        .get_or_insert_with(|| server.load(format!("{MPQ_SOURCE}://world/maps/{dir}/{dir}.wdt")))
        .clone();
    let Some(index) = wdts.get(&wdt) else {
        streamer.wanted.clear();
        return;
    };
    streamer.indexed = true;
    let [x, y, _] = crate::coords::bevy_to_wow(camera.translation);
    let window = Window::at(FARCLIP, x, y);

    let released: Vec<(u32, u32)> = streamer
        .tiles
        .keys()
        .copied()
        .filter(|&key| !window.keeps(key))
        .collect();
    for key in released {
        release(&mut commands, streamer, key);
    }

    streamer.wanted = window
        .tiles()
        .filter(|&(x, y)| index.has_tile(x, y))
        .collect();
    for &(tx, ty) in &streamer.wanted {
        let requests = &mut streamer.requests;
        streamer.tiles.entry((tx, ty)).or_insert_with(|| {
            *requests += 1;
            Tile {
                state: TileState::Unspawned(server.load(format!(
                    "{MPQ_SOURCE}://world/maps/{dir}/{dir}_{tx}_{ty}.adt"
                ))),
                entities: Vec::new(),
                placements: BTreeSet::new(),
                requested: *requests,
            }
        });
    }

    for tile in streamer.tiles.values_mut() {
        if let TileState::Unspawned(handle) = &tile.state
            && tiles.get(handle).is_none()
            && let LoadState::Failed(e) = server.load_state(handle)
        {
            warn!("a tile's collision failed to load: {e}");
            tile.state = TileState::Failed;
        }
    }
    let every_request_arrived = !streamer.tiles.values().any(|t| t.in_flight(&tiles));
    let mut batch: Vec<(u64, (u32, u32))> = Vec::new();
    if every_request_arrived {
        batch.extend(
            streamer
                .tiles
                .iter()
                .filter(|(_, t)| matches!(t.state, TileState::Unspawned(_)))
                .map(|(&key, t)| (t.requested, key)),
        );
    }
    batch.sort_unstable();
    for (_, key) in batch {
        let Some(tile) = streamer.tiles.get_mut(&key) else {
            continue;
        };
        let TileState::Unspawned(handle) = &tile.state else {
            continue;
        };
        let handle = handle.clone();
        let Some(tc) = tiles.get(&handle) else {
            continue;
        };
        tile.entities = spawn_tile(&mut commands, tc);
        tile.state = TileState::Built(handle);
        for d in &tc.doodads {
            register_doodad(streamer, &server, d, key);
        }
        for w in &tc.wmos {
            register_wmo(streamer, &server, w, key);
        }
    }
}

fn release(commands: &mut Commands<'_, '_>, streamer: &mut CollisionStreamer, key: (u32, u32)) {
    let Some(tile) = streamer.tiles.remove(&key) else {
        return;
    };
    despawn_all(commands, tile.entities);
    for uid in tile.placements {
        let gone = streamer.placements.get_mut(&uid).is_some_and(|p| {
            p.refs -= 1;
            p.refs == 0
        });
        if gone && let Some(p) = streamer.placements.remove(&uid) {
            despawn_all(commands, p.entities);
        } else if streamer
            .placements
            .get(&uid)
            .is_some_and(|p| p.owner == key)
        {
            rehome(streamer, uid);
        }
    }
}

fn rehome(streamer: &mut CollisionStreamer, uid: u32) {
    let heir = streamer
        .tiles
        .iter()
        .filter(|(_, t)| t.placements.contains(&uid))
        .min_by_key(|(_, t)| t.requested)
        .map(|(&key, _)| key);
    let (Some(heir), Some(p)) = (heir, streamer.placements.get_mut(&uid)) else {
        return;
    };
    p.owner = heir;
    if let Some(PlacementModel::M2 {
        welded, inherited, ..
    }) = &mut p.model
    {
        *welded = false;
        *inherited = true;
    }
}

fn despawn_all(commands: &mut Commands<'_, '_>, entities: Vec<Entity>) {
    for e in entities {
        commands.entity(e).despawn();
    }
}

fn spawn_tile(commands: &mut Commands<'_, '_>, tc: &TileCollision) -> Vec<Entity> {
    let mut entities = Vec::new();
    if let Some((verts, tris)) = tc.terrain.clone() {
        let task = build_collider_task(verts, tris);
        entities.push(
            commands
                .spawn((
                    Transform::IDENTITY,
                    PendingCollider::new(task, None),
                    GroundDecalSurface,
                ))
                .id(),
        );
    }
    if let Some((verts, tris)) = tc.walls.clone() {
        let task = build_collider_task(verts, tris);
        entities.push(
            commands
                .spawn((
                    Transform::IDENTITY,
                    PendingCollider::new(task, Some(walk_layers())),
                ))
                .id(),
        );
    }
    for liquid in &tc.liquids {
        let grid = world_grid(liquid, &Transform::IDENTITY, LiquidSource::AdtChunk);
        let mut entity = commands.spawn((Transform::IDENTITY, SwimSurface(grid)));
        if let Some(collider) = waterline_collider(liquid, &Transform::IDENTITY) {
            entity.insert((collider, liquid_layers()));
        }
        entities.push(entity.id());
    }
    entities
}

fn spawn_wmo_liquids(
    commands: &mut Commands<'_, '_>,
    hull: &Handle<WmoHull>,
    wmo: &WmoHull,
    at: &Transform,
) -> Vec<Entity> {
    let rooms = &wmo.rooms;
    let instance = commands
        .spawn(PlacedRooms {
            hull: hull.clone(),
            world_from_local: at.compute_affine(),
        })
        .id();
    let mut entities = vec![instance];
    for (gi, liquid) in rooms.group_liquids.iter().enumerate() {
        let Some(liquid) = liquid else { continue };
        let pool = WmoPool::of(rooms, gi, instance, at);
        let grid = world_grid(liquid, at, LiquidSource::WmoGroup(pool));
        let mut entity = commands.spawn((Transform::IDENTITY, SwimSurface(grid)));
        if let Some(collider) = waterline_collider(liquid, at) {
            entity.insert((collider, liquid_layers()));
        }
        entities.push(entity.id());
    }
    entities
}

impl CollisionStreamer {
    pub(super) fn terrain_z_under(&self, tiles: &Assets<TileCollision>, at: Vec3) -> Option<f32> {
        let wow = bevy_to_wow(at);
        let key = wdt::world_to_tile(wow[0], wow[1]);
        let TileState::Built(handle) = &self.tiles.get(&key)?.state else {
            return None;
        };
        terrain::terrain_height_at(&tiles.get(handle)?.ground, wow)
    }
}

fn claim(streamer: &mut CollisionStreamer, uid: u32, tile: (u32, u32)) -> bool {
    let Some(t) = streamer.tiles.get_mut(&tile) else {
        return false;
    };
    if !t.placements.insert(uid) {
        return false;
    }
    if let Some(p) = streamer.placements.get_mut(&uid) {
        p.refs += 1;
        return false;
    }
    true
}

fn register_doodad(
    streamer: &mut CollisionStreamer,
    server: &AssetServer,
    d: &Doodad,
    tile: (u32, u32),
) {
    if !claim(streamer, d.unique_id, tile) {
        return;
    }
    streamer.placements.insert(
        d.unique_id,
        Placement {
            model: Some(PlacementModel::M2 {
                hull: server.load(m2_url(&d.model)),
                welded: false,
                inherited: false,
            }),
            transform: Transform {
                translation: wow_to_bevy(d.position),
                rotation: placement_rotation(d.rotation),
                scale: Vec3::splat(d.scale),
            },
            refs: 1,
            owner: tile,
            entities: Vec::new(),
            unwelded_props: Vec::new(),
        },
    );
}

fn register_wmo(
    streamer: &mut CollisionStreamer,
    server: &AssetServer,
    w: &WmoInstance,
    tile: (u32, u32),
) {
    if !claim(streamer, w.unique_id, tile) {
        return;
    }
    streamer.placements.insert(
        w.unique_id,
        Placement {
            model: Some(PlacementModel::Wmo {
                hull: server.load(wmo_url(&w.model)),
                doodad_set: w.doodad_set,
            }),
            transform: Transform {
                translation: wow_to_bevy(w.position),
                rotation: placement_rotation(w.rotation),
                scale: Vec3::ONE,
            },
            refs: 1,
            owner: tile,
            entities: Vec::new(),
            unwelded_props: Vec::new(),
        },
    );
}

pub(super) fn spawn_placement_colliders(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Hull>>,
    wmos: Res<'_, Assets<WmoHull>>,
    mut streamer: ResMut<'_, CollisionStreamer>,
) {
    for p in streamer.placements.values_mut() {
        if let Some(PlacementModel::Wmo { hull, doodad_set }) = &p.model {
            if let Some(wmo) = wmos.get(hull) {
                p.entities
                    .extend(spawn_wmo(&mut commands, wmo, &p.transform));
                p.entities
                    .extend(spawn_wmo_liquids(&mut commands, hull, wmo, &p.transform));
                p.unwelded_props = wmo_props(&server, wmo, *doodad_set, p.transform);
                p.model = None;
            } else if server.load_state(hull).is_failed() {
                p.model = None;
            }
        }
        weld_props(&mut commands, &server, &m2s, p);
    }
    weld_owned_doodads(&mut commands, &server, &m2s, &mut streamer);
}

fn resolved(server: &AssetServer, m2s: &Assets<M2Hull>, hull: &Handle<M2Hull>) -> bool {
    m2s.contains(hull) || server.load_state(hull).is_failed()
}

fn hull_data(m2s: &Assets<M2Hull>, hull: &Handle<M2Hull>, at: &Transform) -> Option<Trimesh> {
    placement_collider_data(m2s.get(hull)?.0.as_ref(), at)
}

fn weld_props(
    commands: &mut Commands<'_, '_>,
    server: &AssetServer,
    m2s: &Assets<M2Hull>,
    p: &mut Placement,
) {
    if p.unwelded_props.is_empty()
        || !p
            .unwelded_props
            .iter()
            .all(|(hull, _)| resolved(server, m2s, hull))
    {
        return;
    }
    let hulls = p
        .unwelded_props
        .drain(..)
        .filter_map(|(hull, at)| hull_data(m2s, &hull, &at));
    p.entities
        .extend(weld(hulls).into_iter().map(|b| spawn_batch(commands, b)));
}

fn weld_owned_doodads(
    commands: &mut Commands<'_, '_>,
    server: &AssetServer,
    m2s: &Assets<M2Hull>,
    streamer: &mut CollisionStreamer,
) {
    let CollisionStreamer {
        tiles, placements, ..
    } = streamer;
    let mut groups: BTreeMap<((u32, u32), bool), Vec<u32>> = BTreeMap::new();
    for (&uid, p) in placements.iter() {
        if let Some(PlacementModel::M2 {
            welded: false,
            inherited,
            ..
        }) = p.model
        {
            groups.entry((p.owner, inherited)).or_default().push(uid);
        }
    }
    for ((owner, _), uids) in groups {
        let Some(tile) = tiles.get_mut(&owner) else {
            continue;
        };
        let doodad = |uid: &u32| match placements.get(uid) {
            Some(Placement {
                model: Some(PlacementModel::M2 { hull, .. }),
                transform,
                ..
            }) => Some((hull, transform)),
            _ => None,
        };
        if !uids
            .iter()
            .filter_map(doodad)
            .all(|(hull, _)| resolved(server, m2s, hull))
        {
            continue;
        }
        let hulls = uids
            .iter()
            .filter_map(doodad)
            .filter_map(|(hull, at)| hull_data(m2s, hull, at));
        tile.entities
            .extend(weld(hulls).into_iter().map(|b| spawn_batch(commands, b)));
        for uid in &uids {
            if let Some(Placement {
                model: Some(PlacementModel::M2 { welded, .. }),
                ..
            }) = placements.get_mut(uid)
            {
                *welded = true;
            }
        }
    }
}

fn spawn_wmo(commands: &mut Commands<'_, '_>, wmo: &WmoHull, at: &Transform) -> Vec<Entity> {
    [
        (wmo.walk.as_ref(), walk_layers(), true),
        (wmo.camera.as_ref(), camera_layers(), false),
    ]
    .into_iter()
    .filter_map(|(mesh, layers, takes_decals)| {
        let (verts, tris) = placement_collider_data(mesh, at)?;
        let task = build_collider_task(verts, tris);
        let mut spawned = commands.spawn((
            Transform::IDENTITY,
            PendingCollider::new(task, Some(layers)),
        ));
        if takes_decals {
            spawned.insert(GroundDecalSurface);
        }
        Some(spawned.id())
    })
    .collect()
}

/// Set 0 always, and the one set the placement names.
fn wmo_props(
    server: &AssetServer,
    wmo: &WmoHull,
    doodad_set: u16,
    at: Transform,
) -> Vec<(Handle<M2Hull>, Transform)> {
    let mut sets: Vec<usize> = vec![0];
    if doodad_set != 0 {
        sets.push(usize::from(doodad_set));
    }
    sets.into_iter()
        .filter_map(|s| wmo.doodad_sets.get(s))
        .flat_map(|set| {
            wmo.doodads
                .iter()
                .skip(set.start as usize)
                .take(set.count as usize)
        })
        .filter(|d| !d.model.is_empty())
        .map(|d| {
            let local = wmo_doodad_local(d.position, d.orientation, d.scale);
            (server.load(m2_url(&d.model)), at.mul_transform(local))
        })
        .collect()
}

pub(super) fn publish_residency(
    streamer: Res<'_, CollisionStreamer>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Hull>>,
    pending: Query<'_, '_, (), With<PendingCollider>>,
    mut residency: ResMut<'_, CollisionResidency>,
) {
    let unspawned_tiles = streamer
        .wanted
        .iter()
        .filter(|key| {
            streamer
                .tiles
                .get(key)
                .is_none_or(|t| matches!(t.state, TileState::Unspawned(_)))
        })
        .count();
    let unwelded: usize = streamer
        .placements
        .values()
        .map(|p| {
            let waiting = |hull: &Handle<M2Hull>| 1 + usize::from(!resolved(&server, &m2s, hull));
            let model = match &p.model {
                None | Some(PlacementModel::M2 { welded: true, .. }) => 0,
                Some(PlacementModel::M2 { hull, .. }) => waiting(hull),
                Some(PlacementModel::Wmo { .. }) => 1,
            };
            model
                + p.unwelded_props
                    .iter()
                    .map(|(hull, _)| waiting(hull))
                    .sum::<usize>()
        })
        .sum();
    residency.pending = unspawned_tiles + unwelded + pending.count();
    residency.indexed = streamer.indexed;
}
