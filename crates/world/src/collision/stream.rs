//! Streams colliders with the terrain's window around the camera. A placement straddling tiles is
//! kept once, by unique id, while any of its tiles is.

use std::collections::{BTreeMap, BTreeSet};

use bevy::asset::LoadState;
use bevy::prelude::*;
use terrain::{Doodad, WmoInstance};

use super::assets::{M2Hull, TileCollision, WmoHull};
use super::colliders::{PendingCollider, build_collider_task, placement_collider_data};
use super::edits::Edited;
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
/// known to be missing, every tile it let go is gone, every placement's hull is in, and nothing
/// waits to attach.
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

    /// Tiles still to come or go, and hulls, welds and colliders still on their way.
    pub fn pending(&self) -> usize {
        self.pending
    }
}

pub(super) type Change = u64;
pub(super) type TileId = Change;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum WeldGroup {
    Own,
    PassedOnAt(Change),
}

enum TileState {
    Unspawned(Handle<TileCollision>),
    Built(Handle<TileCollision>),
    Failed,
}

pub(super) struct Tile {
    coords: (u32, u32),
    state: TileState,
    entities: Vec<Entity>,
    pub(super) welds: BTreeMap<WeldGroup, Vec<Entity>>,
    placements: BTreeSet<u32>,
    let_go: Option<Change>,
}

impl Tile {
    fn in_flight(&self, tiles: &Assets<TileCollision>) -> bool {
        matches!(&self.state, TileState::Unspawned(handle) if tiles.get(handle).is_none())
    }
}

pub(super) enum Hull<A: Asset> {
    Unasked(String),
    Asked(Handle<A>),
}

impl<A: Asset> Hull<A> {
    fn ask(&mut self, server: &AssetServer) {
        if let Self::Unasked(url) = self {
            *self = Self::Asked(server.load(std::mem::take(url)));
        }
    }

    fn handle(&self) -> Option<&Handle<A>> {
        match self {
            Self::Asked(handle) => Some(handle),
            Self::Unasked(_) => None,
        }
    }
}

pub(super) enum PlacementModel {
    M2 {
        hull: Hull<M2Hull>,
        welded: bool,
        group: WeldGroup,
    },
    Wmo {
        hull: Hull<WmoHull>,
        doodad_set: u16,
    },
}

pub(super) struct Placement {
    pub(super) model: Option<PlacementModel>,
    pub(super) transform: Transform,
    refs: u32,
    pub(super) owner: Option<TileId>,
    pub(super) entities: Vec<Entity>,
    pub(super) unwelded_props: Vec<(Handle<M2Hull>, Transform)>,
}

impl Placement {
    pub(super) fn unowned(model: PlacementModel, transform: Transform) -> Self {
        Self {
            model: Some(model),
            transform,
            refs: 0,
            owner: None,
            entities: Vec::new(),
            unwelded_props: Vec::new(),
        }
    }
}

enum Step {
    Arrive(TileId),
    LetGo { asked: TileId, at: Change },
}

#[derive(Resource, Default)]
pub(crate) struct CollisionStreamer {
    wdt: Option<Handle<WdtIndex>>,
    indexed: bool,
    pub(super) tiles_by_ask: BTreeMap<TileId, Tile>,
    pub(super) placements: BTreeMap<u32, Placement>,
    pub(super) edits: BTreeMap<u32, Edited>,
    wanted: Vec<(u32, u32)>,
    last_change: Change,
}

fn next_change(last: &mut Change) -> Change {
    *last += 1;
    *last
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

    for tile in streamer.tiles_by_ask.values_mut() {
        if tile.let_go.is_none() && !window.keeps(tile.coords) {
            tile.let_go = Some(next_change(&mut streamer.last_change));
        }
    }
    streamer.wanted = window
        .tiles()
        .filter(|&(x, y)| index.has_tile(x, y))
        .collect();
    for &coords in &streamer.wanted {
        if streamer.held(coords).is_none() {
            let (tx, ty) = coords;
            let tile = Tile {
                coords,
                state: TileState::Unspawned(server.load(format!(
                    "{MPQ_SOURCE}://world/maps/{dir}/{dir}_{tx}_{ty}.adt"
                ))),
                entities: Vec::new(),
                welds: BTreeMap::new(),
                placements: BTreeSet::new(),
                let_go: None,
            };
            let asked = next_change(&mut streamer.last_change);
            streamer.tiles_by_ask.insert(asked, tile);
        }
    }

    for tile in streamer.tiles_by_ask.values_mut() {
        if let TileState::Unspawned(handle) = &tile.state
            && tiles.get(handle).is_none()
            && let LoadState::Failed(e) = server.load_state(handle)
        {
            warn!("a tile's collision failed to load: {e}");
            tile.state = TileState::Failed;
        }
    }
    if !streamer.tiles_by_ask.values().any(|t| t.in_flight(&tiles)) {
        apply_changes(&mut commands, &tiles, streamer);
    }
}

fn apply_changes(
    commands: &mut Commands<'_, '_>,
    tiles: &Assets<TileCollision>,
    streamer: &mut CollisionStreamer,
) {
    let mut steps: BTreeMap<Change, Step> = BTreeMap::new();
    for (&asked, tile) in &streamer.tiles_by_ask {
        if matches!(tile.state, TileState::Unspawned(_)) {
            steps.insert(asked, Step::Arrive(asked));
        }
        if let Some(at) = tile.let_go {
            steps.insert(at, Step::LetGo { asked, at });
        }
    }
    for step in steps.into_values() {
        match step {
            Step::Arrive(asked) => arrive(commands, tiles, streamer, asked),
            Step::LetGo { asked, at } => release(commands, streamer, asked, at),
        }
    }
}

fn arrive(
    commands: &mut Commands<'_, '_>,
    tiles: &Assets<TileCollision>,
    streamer: &mut CollisionStreamer,
    asked: TileId,
) {
    let Some(tile) = streamer.tiles_by_ask.get_mut(&asked) else {
        return;
    };
    let TileState::Unspawned(handle) = &tile.state else {
        return;
    };
    let handle = handle.clone();
    let Some(tc) = tiles.get(&handle) else {
        return;
    };
    if tile.let_go.is_none() {
        tile.entities = spawn_tile(commands, tc);
    }
    tile.state = TileState::Built(handle);
    for d in &tc.doodads {
        register_doodad(streamer, d, asked);
    }
    for w in &tc.wmos {
        register_wmo(streamer, w, asked);
    }
}

fn release(
    commands: &mut Commands<'_, '_>,
    streamer: &mut CollisionStreamer,
    asked: TileId,
    at: Change,
) {
    let Some(tile) = streamer.tiles_by_ask.remove(&asked) else {
        return;
    };
    despawn_all(commands, tile.entities);
    despawn_all(commands, tile.welds.into_values().flatten().collect());
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
            .is_some_and(|p| p.owner == Some(asked))
        {
            pass_on(streamer, uid, at);
        }
    }
}

fn pass_on(streamer: &mut CollisionStreamer, uid: u32, at: Change) {
    let heir = streamer
        .tiles_by_ask
        .iter()
        .find(|(_, t)| t.placements.contains(&uid))
        .map(|(&asked, _)| asked);
    let (Some(heir), Some(p)) = (heir, streamer.placements.get_mut(&uid)) else {
        return;
    };
    p.owner = Some(heir);
    if let Some(PlacementModel::M2 { welded, group, .. }) = &mut p.model {
        *welded = false;
        *group = WeldGroup::PassedOnAt(at);
    }
}

pub(super) fn despawn_all(commands: &mut Commands<'_, '_>, entities: Vec<Entity>) {
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
        let coords = wdt::world_to_tile(wow[0], wow[1]);
        let handle = self.tiles_by_ask.values().find_map(|t| match &t.state {
            TileState::Built(handle) if t.coords == coords => Some(handle),
            _ => None,
        })?;
        terrain::terrain_height_at(&tiles.get(handle)?.ground, wow)
    }

    fn held(&self, coords: (u32, u32)) -> Option<&Tile> {
        self.tiles_by_ask
            .values()
            .find(|t| t.coords == coords && t.let_go.is_none())
    }
}

fn claim(streamer: &mut CollisionStreamer, uid: u32, tile: TileId) -> bool {
    let Some(t) = streamer.tiles_by_ask.get_mut(&tile) else {
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

fn register_doodad(streamer: &mut CollisionStreamer, d: &Doodad, tile: TileId) {
    if !claim(streamer, d.unique_id, tile) {
        return;
    }
    streamer.placements.insert(
        d.unique_id,
        Placement {
            model: Some(PlacementModel::M2 {
                hull: Hull::Unasked(m2_url(&d.model)),
                welded: false,
                group: WeldGroup::Own,
            }),
            transform: Transform {
                translation: wow_to_bevy(d.position),
                rotation: placement_rotation(d.rotation),
                scale: Vec3::splat(d.scale),
            },
            refs: 1,
            owner: Some(tile),
            entities: Vec::new(),
            unwelded_props: Vec::new(),
        },
    );
}

fn register_wmo(streamer: &mut CollisionStreamer, w: &WmoInstance, tile: TileId) {
    if !claim(streamer, w.unique_id, tile) {
        return;
    }
    streamer.placements.insert(
        w.unique_id,
        Placement {
            model: Some(PlacementModel::Wmo {
                hull: Hull::Unasked(wmo_url(&w.model)),
                doodad_set: w.doodad_set,
            }),
            transform: Transform {
                translation: wow_to_bevy(w.position),
                rotation: placement_rotation(w.rotation),
                scale: Vec3::ONE,
            },
            refs: 1,
            owner: Some(tile),
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
    let (m2s, wmos) = (&*m2s, &*wmos);
    let CollisionStreamer {
        placements, edits, ..
    } = &mut *streamer;
    for (_, p) in unedited_tile_placements_mut(placements, edits) {
        spawn_ready(&mut commands, &server, (m2s, wmos), p);
    }
    for p in edits.values_mut().filter_map(Edited::placement) {
        spawn_ready(&mut commands, &server, (m2s, wmos), p);
        if let Some(PlacementModel::M2 {
            hull: Hull::Asked(hull),
            ..
        }) = &p.model
            && resolved(&server, m2s, hull)
        {
            p.entities.extend(
                hull_data(m2s, hull, &p.transform).map(|data| spawn_batch(&mut commands, data)),
            );
            p.model = None;
        }
    }
    weld_owned_doodads(&mut commands, &server, m2s, &mut streamer);
}

fn spawn_ready(
    commands: &mut Commands<'_, '_>,
    server: &AssetServer,
    (m2s, wmos): (&Assets<M2Hull>, &Assets<WmoHull>),
    p: &mut Placement,
) {
    match &mut p.model {
        Some(PlacementModel::M2 { hull, .. }) => hull.ask(server),
        Some(PlacementModel::Wmo { hull, .. }) => hull.ask(server),
        None => {}
    }
    if let Some(PlacementModel::Wmo {
        hull: Hull::Asked(hull),
        doodad_set,
    }) = &p.model
    {
        if let Some(wmo) = wmos.get(hull) {
            p.entities.extend(spawn_wmo(commands, wmo, &p.transform));
            p.entities
                .extend(spawn_wmo_liquids(commands, hull, wmo, &p.transform));
            p.unwelded_props = wmo_props(server, wmo, *doodad_set, p.transform);
            p.model = None;
        } else if server.load_state(hull).is_failed() {
            p.model = None;
        }
    }
    weld_props(commands, server, m2s, p);
}

fn unedited_tile_placements<'a>(
    placements: &'a BTreeMap<u32, Placement>,
    edits: &'a BTreeMap<u32, Edited>,
) -> impl Iterator<Item = (u32, &'a Placement)> {
    placements
        .iter()
        .filter(|(uid, _)| !edits.contains_key(uid))
        .map(|(&uid, p)| (uid, p))
}

fn unedited_tile_placements_mut<'a>(
    placements: &'a mut BTreeMap<u32, Placement>,
    edits: &'a BTreeMap<u32, Edited>,
) -> impl Iterator<Item = (u32, &'a mut Placement)> {
    placements
        .iter_mut()
        .filter(|(uid, _)| !edits.contains_key(uid))
        .map(|(&uid, p)| (uid, p))
}

pub(super) fn resolved(server: &AssetServer, m2s: &Assets<M2Hull>, hull: &Handle<M2Hull>) -> bool {
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
        tiles_by_ask,
        placements,
        edits,
        ..
    } = streamer;
    let mut groups: BTreeMap<(TileId, WeldGroup), Vec<u32>> = BTreeMap::new();
    for (uid, p) in unedited_tile_placements(placements, edits) {
        if let (
            Some(owner),
            Some(PlacementModel::M2 {
                welded: false,
                group,
                ..
            }),
        ) = (p.owner, &p.model)
        {
            groups.entry((owner, *group)).or_default().push(uid);
        }
    }
    for ((owner, group), uids) in groups {
        let Some(tile) = tiles_by_ask.get_mut(&owner).filter(|t| t.let_go.is_none()) else {
            continue;
        };
        let doodad = |uid: &u32| match placements.get(uid) {
            Some(Placement {
                model: Some(PlacementModel::M2 { hull, .. }),
                transform,
                ..
            }) => Some((hull.handle(), transform)),
            _ => None,
        };
        if !uids
            .iter()
            .filter_map(doodad)
            .all(|(hull, _)| hull.is_some_and(|hull| resolved(server, m2s, hull)))
        {
            continue;
        }
        let hulls = uids
            .iter()
            .filter_map(doodad)
            .filter_map(|(hull, at)| hull_data(m2s, hull?, at));
        tile.welds
            .entry(group)
            .or_default()
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
        .filter(|&&coords| {
            streamer
                .held(coords)
                .is_none_or(|t| matches!(t.state, TileState::Unspawned(_)))
        })
        .count();
    let leaving_tiles = streamer
        .tiles_by_ask
        .values()
        .filter(|t| t.let_go.is_some())
        .count();
    let unwelded: usize = unedited_tile_placements(&streamer.placements, &streamer.edits)
        .map(|(_, p)| p)
        .chain(streamer.edits.values().filter_map(Edited::standing))
        .map(|p| {
            let waiting = |hull: Option<&Handle<M2Hull>>| {
                1 + usize::from(hull.is_none_or(|hull| !resolved(&server, &m2s, hull)))
            };
            let model = match &p.model {
                None | Some(PlacementModel::M2 { welded: true, .. }) => 0,
                Some(PlacementModel::M2 { hull, .. }) => waiting(hull.handle()),
                Some(PlacementModel::Wmo { .. }) => 1,
            };
            model
                + p.unwelded_props
                    .iter()
                    .map(|(hull, _)| waiting(Some(hull)))
                    .sum::<usize>()
        })
        .sum();
    residency.pending = unspawned_tiles + leaving_tiles + unwelded + pending.count();
    residency.indexed = streamer.indexed;
}
