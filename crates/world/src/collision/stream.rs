//! Streams colliders with the terrain's window around the camera. A placement straddling tiles is
//! kept once, by unique id, while any of its tiles is.

use std::collections::BTreeMap;

use bevy::asset::LoadState;
use bevy::prelude::*;
use terrain::{Doodad, WmoInstance};

use super::assets::{M2Hull, TileCollision, WmoHull};
use super::colliders::{PendingCollider, build_collider_task, placement_collider_data};
use super::liquid::{PlacedRooms, SwimSurface, waterline_collider};
use super::weld::HullWelds;
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
    Loading(Handle<TileCollision>),
    Built(Handle<TileCollision>),
    Failed,
}

pub(super) struct Tile {
    state: TileState,
    pub(super) entities: Vec<Entity>,
    placements: Vec<u32>,
}

enum PlacementModel {
    M2(Handle<M2Hull>),
    Wmo {
        hull: Handle<WmoHull>,
        doodad_set: u16,
    },
}

pub(super) struct Placement {
    model: Option<PlacementModel>,
    transform: Transform,
    refs: u32,
    owner: (u32, u32),
    pub(super) entities: Vec<Entity>,
    /// A building's props whose hulls are still loading, at their world transforms.
    props: Vec<(Handle<M2Hull>, Transform)>,
}

#[derive(Resource, Default)]
pub(crate) struct CollisionStreamer {
    wdt: Option<Handle<WdtIndex>>,
    indexed: bool,
    pub(super) tiles: BTreeMap<(u32, u32), Tile>,
    pub(super) placements: BTreeMap<u32, Placement>,
    pub(super) welds: HullWelds,
    wanted: Vec<(u32, u32)>,
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
        let Some(tile) = streamer.tiles.remove(&key) else {
            continue;
        };
        despawn_all(&mut commands, tile.entities);
        for uid in tile.placements {
            let gone = streamer.placements.get_mut(&uid).is_some_and(|p| {
                p.refs -= 1;
                p.refs == 0
            });
            if gone && let Some(p) = streamer.placements.remove(&uid) {
                despawn_all(&mut commands, p.entities);
            }
        }
    }

    streamer.wanted = window
        .tiles()
        .filter(|&(x, y)| index.has_tile(x, y))
        .collect();
    for &(tx, ty) in &streamer.wanted {
        streamer.tiles.entry((tx, ty)).or_insert_with(|| Tile {
            state: TileState::Loading(server.load(format!(
                "{MPQ_SOURCE}://world/maps/{dir}/{dir}_{tx}_{ty}.adt"
            ))),
            entities: Vec::new(),
            placements: Vec::new(),
        });
    }

    let keys: Vec<(u32, u32)> = streamer.tiles.keys().copied().collect();
    for key in keys {
        let Some(tile) = streamer.tiles.get_mut(&key) else {
            continue;
        };
        let TileState::Loading(handle) = &tile.state else {
            continue;
        };
        let handle = handle.clone();
        if let Some(tc) = tiles.get(&handle) {
            tile.entities = spawn_tile(&mut commands, tc);
            tile.state = TileState::Built(handle.clone());
            for d in &tc.doodads {
                register_doodad(streamer, &server, d, key);
            }
            for w in &tc.wmos {
                register_wmo(streamer, &server, w, key);
            }
        } else if let LoadState::Failed(e) = server.load_state(&handle) {
            warn!("a tile's collision failed to load: {e}");
            tile.state = TileState::Failed;
        }
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
    t.placements.push(uid);
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
            model: Some(PlacementModel::M2(server.load(m2_url(&d.model)))),
            transform: Transform {
                translation: wow_to_bevy(d.position),
                rotation: placement_rotation(d.rotation),
                scale: Vec3::splat(d.scale),
            },
            refs: 1,
            owner: tile,
            entities: Vec::new(),
            props: Vec::new(),
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
            props: Vec::new(),
        },
    );
}

/// Turns loaded hulls into colliders.
pub(super) fn spawn_placement_colliders(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Hull>>,
    wmos: Res<'_, Assets<WmoHull>>,
    mut streamer: ResMut<'_, CollisionStreamer>,
) {
    let streamer = &mut *streamer;
    for (&uid, p) in &mut streamer.placements {
        match &p.model {
            Some(PlacementModel::M2(handle)) => {
                if let Some(hull) = m2s.get(handle) {
                    if let Some((verts, tris)) =
                        placement_collider_data(hull.0.as_ref(), &p.transform)
                    {
                        streamer.welds.add_tile(p.owner, verts, tris);
                    }
                    p.model = None;
                } else if server.load_state(handle).is_failed() {
                    p.model = None;
                }
            }
            Some(PlacementModel::Wmo { hull, doodad_set }) => {
                if let Some(wmo) = wmos.get(hull) {
                    p.entities
                        .extend(spawn_wmo(&mut commands, wmo, &p.transform));
                    p.entities
                        .extend(spawn_wmo_liquids(&mut commands, hull, wmo, &p.transform));
                    p.props = wmo_props(&server, wmo, *doodad_set, p.transform);
                    p.model = None;
                } else if server.load_state(hull).is_failed() {
                    p.model = None;
                }
            }
            None => {}
        }
        p.props.retain(|(handle, transform)| {
            if let Some(hull) = m2s.get(handle) {
                if let Some((verts, tris)) = placement_collider_data(hull.0.as_ref(), transform) {
                    streamer.welds.add_prop(uid, verts, tris);
                }
                return false;
            }
            !server.load_state(handle).is_failed()
        });
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
    pending: Query<'_, '_, (), With<PendingCollider>>,
    mut residency: ResMut<'_, CollisionResidency>,
) {
    let loading_tiles = streamer
        .wanted
        .iter()
        .filter(|key| {
            streamer
                .tiles
                .get(key)
                .is_none_or(|t| matches!(t.state, TileState::Loading(_)))
        })
        .count();
    let loading_hulls: usize = streamer
        .placements
        .values()
        .map(|p| usize::from(p.model.is_some()) + p.props.len())
        .sum();
    residency.pending =
        loading_tiles + loading_hulls + streamer.welds.unflushed() + pending.count();
    residency.indexed = streamer.indexed;
}
