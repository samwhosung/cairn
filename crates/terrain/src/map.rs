use std::io::Cursor;

use mpq::Chain;
use wdt::{WdtFile, WdtReader, world_to_tile};

use crate::Error;
use crate::mesh::{TileMesh, adt_to_tile_mesh};

const NEAR_SEARCH_RADIUS: u32 = 4;

type PlacedTile = ((u32, u32), TileMesh);

struct Candidate {
    distance_sq: i32,
    tile: (u32, u32),
}

fn read(chain: &Chain, path: String) -> Result<Vec<u8>, Error> {
    chain
        .read(&path)
        .map_err(|source| Error::Read { path, source })
}

fn read_wdt(chain: &Chain, map: &str) -> Result<WdtFile, Error> {
    let path = format!("World\\Maps\\{map}\\{map}.wdt");
    let bytes = read(chain, path.clone())?;
    WdtReader::new(Cursor::new(bytes))
        .read()
        .map_err(|source| Error::Wdt { path, source })
}

fn tile_exists(wdt: &WdtFile, x: i32, y: i32) -> bool {
    (0..64).contains(&x)
        && (0..64).contains(&y)
        && wdt
            .get_tile(x as usize, y as usize)
            .is_some_and(|t| t.has_adt)
}

fn tiles_in_square(wdt: &WdtFile, (cx, cy): (u32, u32), radius: u32) -> Vec<Candidate> {
    let r = radius as i32;
    let mut out = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
            if tile_exists(wdt, tx, ty) {
                out.push(Candidate {
                    distance_sq: dx * dx + dy * dy,
                    tile: (tx as u32, ty as u32),
                });
            }
        }
    }
    out
}

/// The tile `(x, y)`, as in `Map_<x>_<y>.adt`, holding world `(world_x, world_y)` on `map`, or
/// when it has no terrain the first that does in growing squares around it, up to four tiles out.
pub fn find_tile_near(
    chain: &Chain,
    map: &str,
    world_x: f32,
    world_y: f32,
) -> Result<(u32, u32), Error> {
    let wdt = read_wdt(chain, map)?;
    let centre = world_to_tile(world_x, world_y);
    (0..=NEAR_SEARCH_RADIUS)
        .find_map(|r| tiles_in_square(&wdt, centre, r).first().map(|c| c.tile))
        .ok_or_else(|| Error::NoTileNear {
            map: map.to_string(),
            x: world_x,
            y: world_y,
        })
}

/// A map's tile table, read once for repeated queries.
pub struct MapTiles {
    map: String,
    wdt: WdtFile,
}

impl MapTiles {
    pub fn load(chain: &Chain, map: &str) -> Result<Self, Error> {
        Ok(Self {
            map: map.to_string(),
            wdt: read_wdt(chain, map)?,
        })
    }

    pub fn map(&self) -> &str {
        &self.map
    }

    pub fn tile_at(&self, world_x: f32, world_y: f32) -> (u32, u32) {
        world_to_tile(world_x, world_y)
    }

    /// Every tile with terrain within `radius` tiles of world `(world_x, world_y)`, nearest
    /// first.
    pub fn existing_in_radius(&self, world_x: f32, world_y: f32, radius: u32) -> Vec<(u32, u32)> {
        let mut tiles = tiles_in_square(&self.wdt, world_to_tile(world_x, world_y), radius);
        tiles.sort_by_key(|c| c.distance_sq);
        tiles.into_iter().map(|c| c.tile).collect()
    }
}

pub fn load_tile_mesh(
    chain: &Chain,
    map: &str,
    tile_x: u32,
    tile_y: u32,
) -> Result<TileMesh, Error> {
    let bytes = read(
        chain,
        format!("World\\Maps\\{map}\\{map}_{tile_x}_{tile_y}.adt"),
    )?;
    adt_to_tile_mesh(&bytes)
}

/// Every tile within `radius` tiles of world `(world_x, world_y)` that exists and builds, row by
/// row; a tile that fails is skipped. Fails only when none builds.
pub fn load_tiles_around(
    chain: &Chain,
    map: &str,
    world_x: f32,
    world_y: f32,
    radius: u32,
) -> Result<Vec<PlacedTile>, Error> {
    let wdt = read_wdt(chain, map)?;
    let out: Vec<PlacedTile> = tiles_in_square(&wdt, world_to_tile(world_x, world_y), radius)
        .into_iter()
        .filter_map(|Candidate { tile: (x, y), .. }| {
            Some(((x, y), load_tile_mesh(chain, map, x, y).ok()?))
        })
        .collect();
    if out.is_empty() {
        return Err(Error::NoTilesAround {
            map: map.to_string(),
            x: world_x,
            y: world_y,
            radius,
        });
    }
    Ok(out)
}
