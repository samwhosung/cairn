use std::collections::BTreeMap;

use bevy::asset::LoadState;
use bevy::prelude::*;
use terrain::CHUNK_SIZE;

use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::light::LightBuffer;
use crate::liquid::{LiquidAssets, spawn_adt_liquids};
use crate::source::MPQ_SOURCE;
use crate::terrain::{TerrainMaterial, terrain_material};
use crate::view::{FARCLIP, WorldCamera};
use crate::wdt::WdtIndex;
use crate::{CurrentMap, Residency};

const CHUNKS_PER_TILE: i32 = 16;
const LAST_CHUNK: i32 = 64 * CHUNKS_PER_TILE - 1;
/// The client keeps at least this many chunks around the camera, however near the far clip.
const MIN_REACH_CHUNKS: i32 = 8;
const RELEASE_HYSTERESIS_CHUNKS: i32 = 1;

/// The client's chunk window: `1 + farclip / chunk` chunks resident and two more requested.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    focus: (i32, i32),
    reach: i32,
}

impl Window {
    pub(crate) fn at(farclip: f32, wow_x: f32, wow_y: f32) -> Self {
        let (cx, cy) = wdt::world_to_chunk(wow_x, wow_y);
        let inner = 1 + (farclip / CHUNK_SIZE).trunc() as i32;
        Self {
            focus: (cx as i32, cy as i32),
            reach: (inner + 2).max(MIN_REACH_CHUNKS),
        }
    }

    fn tile_range(self, band: i32) -> ((u32, u32), (u32, u32)) {
        let half = self.reach + band;
        let lo = |c: i32| ((c - half).clamp(0, LAST_CHUNK) / CHUNKS_PER_TILE) as u32;
        let hi = |c: i32| ((c + half).clamp(0, LAST_CHUNK) / CHUNKS_PER_TILE) as u32;
        let (fx, fy) = self.focus;
        ((lo(fx), lo(fy)), (hi(fx), hi(fy)))
    }

    pub(crate) fn tiles(self) -> impl Iterator<Item = (u32, u32)> {
        let ((x0, y0), (x1, y1)) = self.tile_range(0);
        (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
    }

    pub(crate) fn keeps(self, (x, y): (u32, u32)) -> bool {
        let ((x0, y0), (x1, y1)) = self.tile_range(RELEASE_HYSTERESIS_CHUNKS);
        (x0..=x1).contains(&x) && (y0..=y1).contains(&y)
    }
}

enum TileState {
    Unspawned,
    Drawn(Vec<Entity>),
    Empty,
    Failed,
}

struct Tile {
    handle: Handle<AdtTile>,
    state: TileState,
    requested: u64,
}

impl Tile {
    fn in_flight(&self, adts: &Assets<AdtTile>) -> bool {
        matches!(self.state, TileState::Unspawned) && adts.get(&self.handle).is_none()
    }
}

#[derive(Resource, Default)]
pub(crate) struct Streamer {
    wdt: Option<Handle<WdtIndex>>,
    tiles: BTreeMap<(u32, u32), Tile>,
    requests: u64,
}

impl Streamer {
    pub(crate) fn pending_or_arrived(&self, tile: (u32, u32)) -> Option<&Handle<AdtTile>> {
        self.tiles
            .get(&tile)
            .filter(|t| !matches!(t.state, TileState::Failed))
            .map(|t| &t.handle)
    }

    pub(crate) fn arrived(&self) -> impl Iterator<Item = ((u32, u32), &Handle<AdtTile>)> {
        self.tiles
            .iter()
            .filter(|(_, t)| matches!(t.state, TileState::Drawn(_) | TileState::Empty))
            .map(|(&key, t)| (key, &t.handle))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_terrain(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    map: Res<'_, CurrentMap>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
    wdts: Res<'_, Assets<WdtIndex>>,
    adts: Res<'_, Assets<AdtTile>>,
    light: Option<Res<'_, LightBuffer>>,
    liquids: Option<Res<'_, LiquidAssets>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<TerrainMaterial>>,
    mut streamer: ResMut<'_, Streamer>,
    mut residency: ResMut<'_, Residency>,
) {
    let (Ok(camera), Some(light), Some(liquids)) = (camera.single(), light, liquids) else {
        return;
    };
    let dir = map.directory.to_ascii_lowercase();
    let wdt = streamer
        .wdt
        .get_or_insert_with(|| server.load(format!("{MPQ_SOURCE}://world/maps/{dir}/{dir}.wdt")))
        .clone();
    let Some(index) = wdts.get(&wdt) else {
        if let LoadState::Failed(e) = server.load_state(&wdt) {
            warn!("no terrain: {e}");
            residency.terrain = true;
        }
        return;
    };
    let [x, y, _] = bevy_to_wow(camera.translation);
    let window = Window::at(FARCLIP, x, y);
    streamer.tiles.retain(|&key, tile| {
        let keep = window.keeps(key);
        if !keep && let TileState::Drawn(entities) = &tile.state {
            for &e in entities {
                commands.entity(e).despawn();
            }
        }
        keep
    });
    let wanted: Vec<(u32, u32)> = window
        .tiles()
        .filter(|&(x, y)| index.has_tile(x, y))
        .collect();
    let Streamer {
        tiles, requests, ..
    } = &mut *streamer;
    for &(tx, ty) in &wanted {
        tiles.entry((tx, ty)).or_insert_with(|| {
            *requests += 1;
            Tile {
                handle: server.load(format!(
                    "{MPQ_SOURCE}://world/maps/{dir}/{dir}_{tx}_{ty}.adt"
                )),
                state: TileState::Unspawned,
                requested: *requests,
            }
        });
    }
    for tile in tiles.values_mut() {
        if tile.in_flight(&adts)
            && let LoadState::Failed(e) = server.load_state(&tile.handle)
        {
            warn!("a terrain tile failed to load: {e}");
            tile.state = TileState::Failed;
        }
    }
    let every_request_arrived = !tiles.values().any(|t| t.in_flight(&adts));
    let mut batch: Vec<(u64, (u32, u32))> = Vec::new();
    if every_request_arrived {
        batch.extend(
            tiles
                .iter()
                .filter(|(_, t)| matches!(t.state, TileState::Unspawned))
                .map(|(&key, t)| (t.requested, key)),
        );
    }
    batch.sort_unstable();
    for (_, key) in batch {
        let Some(tile) = tiles.get_mut(&key) else {
            continue;
        };
        if let Some(adt) = adts.get(&tile.handle) {
            let mut entities = spawn_adt_liquids(
                &mut commands,
                &mut meshes,
                &liquids,
                adt.chunks.iter().flat_map(|c| &c.liquids),
            );
            if let Some((mesh, aabb)) = &adt.mesh {
                let material = materials.add(terrain_material(adt, &light.0));
                entities.push(
                    commands
                        .spawn((
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material),
                            *aabb,
                            Transform::IDENTITY,
                        ))
                        .id(),
                );
            }
            tile.state = if entities.is_empty() {
                TileState::Empty
            } else {
                TileState::Drawn(entities)
            };
        }
    }
    residency.terrain = wanted
        .iter()
        .all(|key| !matches!(tiles[key].state, TileState::Unspawned));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_chunk(farclip: f32, tile: (u32, u32), chunk: (i32, i32)) -> Window {
        let (ox, oy) = wdt::tile_to_world(tile.0, tile.1);
        let wy = oy - (chunk.0 as f32 + 0.5) * CHUNK_SIZE;
        let wx = ox - (chunk.1 as f32 + 0.5) * CHUNK_SIZE;
        Window::at(farclip, wx, wy)
    }

    #[test]
    fn the_reach_is_the_clients() {
        for (farclip, reach) in [(777.0, 26), (350.0, 13), (177.0, 8)] {
            assert_eq!(Window::at(farclip, 0.0, 0.0).reach, reach, "{farclip}");
        }
    }

    #[test]
    fn a_tile_is_wanted_when_the_window_touches_it() {
        let middle = at_chunk(350.0, (32, 48), (7, 7));
        assert_eq!(middle.tiles().count(), 9);
        assert!(
            middle
                .tiles()
                .all(|(x, y)| x.abs_diff(32) <= 1 && y.abs_diff(48) <= 1)
        );
        let corner = at_chunk(350.0, (32, 48), (0, 0));
        assert_eq!(corner.tiles_range_len(), (2, 2));
        assert!(corner.keeps((31, 47)) && !corner.keeps((33, 49)));
    }

    impl Window {
        fn tiles_range_len(self) -> (u32, u32) {
            let ((x0, y0), (x1, y1)) = self.tile_range(0);
            (x1 - x0 + 1, y1 - y0 + 1)
        }
    }
}
