use std::collections::BTreeMap;

use fits::{Evidence, Own, Spot, Tables};
use terrain::TileMesh;
use world::{CurrentMap, Install};

use super::{Borrows, header};
use crate::zone::Zone;

/// The tiles kept for spots on the install, at most those around the spot's own.
const KEPT_TILES_AROUND: u32 = 1;

/// What stands around the spots a list is asked for, and what counts toward the lists.
pub(crate) enum Surroundings {
    Install(OnTheInstall),
    OwnZone(OfItsOwn),
}

pub(crate) struct OnTheInstall {
    install: Install,
    map: u32,
    directory: String,
    areas: atlas::Areas,
    tiles: BTreeMap<(u32, u32), Option<TileMesh>>,
    nothing_of_its_own: Own,
}

pub(crate) struct OfItsOwn {
    directory: String,
    tiles: Vec<((u32, u32), TileMesh)>,
    own: Own,
    unknown: usize,
    palette: Option<usize>,
}

/// A spot, and what the list's header says of it.
pub(crate) struct Found {
    pub(crate) spot: Spot,
    pub(crate) header: String,
    pub(crate) sheet_place: String,
    /// The ground texture that shows most under the spot, as the maps name it.
    pub(crate) under: Option<String>,
}

impl Surroundings {
    pub(crate) fn on_the_install(install: Install, map: &CurrentMap) -> Result<Self, String> {
        let areas = atlas::Areas::load(&install.0)?;
        Ok(Self::Install(OnTheInstall {
            install,
            map: map.id,
            directory: map.directory.clone(),
            areas,
            tiles: BTreeMap::new(),
            nothing_of_its_own: Own::default(),
        }))
    }

    /// Counts what the zone's files place, once.
    pub(crate) fn of_a_zone_of_its_own(
        tables: &Tables,
        install: &Install,
        zone: &Zone,
        borrows: Option<&Borrows>,
    ) -> Result<Self, String> {
        let tiles = atlas::load_map(&install.0, &zone.directory);
        if tiles.is_empty() {
            return Err(format!("no tile of {} reads", zone.directory));
        }
        let Placed { own, unknown } = placed_in(tables, &by_ref(&tiles));
        let borrowed = match borrows {
            Some(Borrows::Nothing) => None,
            Some(Borrows::Zone(name)) => Some(name.as_str()),
            None => Some(zone.borrows.as_str()),
        };
        let palette = borrowed
            .map(|name| {
                tables
                    .zone_named(name)
                    .ok_or_else(|| format!("the catalog has no zone named {name}"))
            })
            .transpose()?;
        Ok(Self::OwnZone(OfItsOwn {
            directory: zone.directory.clone(),
            tiles,
            own,
            unknown,
            palette,
        }))
    }

    pub(crate) fn evidence<'a>(&'a self, tables: &'a Tables) -> Evidence<'a> {
        match self {
            Self::Install(i) => Evidence::new(tables, &i.nothing_of_its_own, true),
            Self::OwnZone(z) => Evidence::new(tables, &z.own, z.palette.is_some()),
        }
    }

    /// The spot at world `at` and what stands around it.
    pub(crate) fn find(&mut self, tables: &Tables, at: [f32; 2]) -> Result<Found, String> {
        match self {
            Self::Install(i) => i.find(tables, at),
            Self::OwnZone(z) => z.find(tables, at),
        }
    }
}

impl OnTheInstall {
    fn find(&mut self, tables: &Tables, at: [f32; 2]) -> Result<Found, String> {
        let wanted = tiles_around(at);
        for &(x, y) in &wanted {
            self.tiles.entry((x, y)).or_insert_with(|| {
                terrain::load_tile_mesh(&self.install.0, &self.directory, x, y).ok()
            });
        }
        let (cx, cy) = wdt::world_to_tile(at[0], at[1]);
        self.tiles
            .retain(|&(x, y), _| x.abs_diff(cx).max(y.abs_diff(cy)) <= KEPT_TILES_AROUND);
        let tiles: Vec<((u32, u32), &TileMesh)> = wanted
            .iter()
            .filter_map(|t| Some((*t, self.tiles.get(t)?.as_ref()?)))
            .collect();
        let here = underfoot(&tiles, at)
            .ok_or_else(|| format!("{},{} has no ground on {}", at[0], at[1], self.directory))?;
        let zone = tables.zone(self.map, self.areas.top_zone(here.area).unwrap_or(0));
        let mut near = Vec::new();
        let mut unknown = 0;
        for s in each_once(&tiles).values() {
            let d = (s.at[0] - at[0]).hypot(s.at[1] - at[1]);
            if d > fits::AROUND {
                continue;
            }
            match tables.model(s.model) {
                Some(m) => near.push((m, d)),
                None => unknown += 1,
            }
        }
        near.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let spot = Spot {
            zone,
            ground: ground_here(tables, &here),
            near,
        };
        let sheet_place = zone
            .map_or("no zone", |z| tables.zones[z].name.as_str())
            .to_owned();
        let place = format!(
            "{},{} on {}, in {sheet_place}",
            at[0], at[1], self.directory
        );
        Ok(Found {
            header: header(tables, &place, &here, &spot, unknown),
            spot,
            sheet_place,
            under: here.texture,
        })
    }
}

impl OfItsOwn {
    fn find(&self, tables: &Tables, at: [f32; 2]) -> Result<Found, String> {
        let here = underfoot(&by_ref(&self.tiles), at)
            .ok_or_else(|| format!("{},{} is off {}'s ground", at[0], at[1], self.directory))?;
        let spot = Spot {
            zone: self.palette,
            ground: ground_here(tables, &here),
            near: self.own.around(at),
        };
        let sheet_place = self.palette.map_or_else(
            || format!("{}, by its own placements alone", self.directory),
            |z| format!("{}, borrowing {}", self.directory, tables.zones[z].name),
        );
        let place = format!(
            "{},{} in {sheet_place}, a zone of its own with {} things",
            at[0],
            at[1],
            self.own.len()
        );
        Ok(Found {
            header: header(tables, &place, &here, &spot, self.unknown),
            spot,
            sheet_place,
            under: here.texture,
        })
    }
}

fn by_ref(tiles: &[((u32, u32), TileMesh)]) -> Vec<((u32, u32), &TileMesh)> {
    tiles.iter().map(|(t, mesh)| (*t, mesh)).collect()
}

fn underfoot(tiles: &[((u32, u32), &TileMesh)], at: [f32; 2]) -> Option<survey::Underfoot> {
    let tile = wdt::world_to_tile(at[0], at[1]);
    let (_, mesh) = tiles.iter().find(|(t, _)| *t == tile)?;
    survey::underfoot(&mesh.chunks, tile, [at[0], at[1], 0.0])
}

struct Placed {
    own: Own,
    unknown: usize,
}

fn placed_in(tables: &Tables, tiles: &[((u32, u32), &TileMesh)]) -> Placed {
    let mut own = Own::default();
    let mut unknown = 0;
    for (id, s) in each_once(tiles) {
        match tables.model(s.model) {
            Some(m) => {
                let ground = s.under.as_ref().and_then(|u| ground_here(tables, u));
                own.place(&id, m, s.at, ground);
            }
            None => unknown += 1,
        }
    }
    Placed { own, unknown }
}

fn ground_here(tables: &Tables, here: &survey::Underfoot) -> Option<(usize, u8)> {
    let texture = tables.ground_texture(here.texture.as_deref()?)?;
    Some((texture, fits::band(here.slope?)))
}

fn tiles_around(at: [f32; 2]) -> Vec<(u32, u32)> {
    let reach = fits::AROUND;
    let mut wanted: Vec<(u32, u32)> = [-reach, reach]
        .iter()
        .flat_map(|dx| [-reach, reach].map(|dy| wdt::world_to_tile(at[0] + dx, at[1] + dy)))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    wanted
}

struct Standing<'a> {
    model: &'a str,
    at: [f32; 2],
    under: Option<survey::Underfoot>,
}

fn each_once<'a>(tiles: &[((u32, u32), &'a TileMesh)]) -> BTreeMap<String, Standing<'a>> {
    let mut by_id: BTreeMap<String, Standing<'a>> = BTreeMap::new();
    for &(tile, mesh) in tiles {
        for (id, model, p) in placed_on(mesh) {
            let under = survey::underfoot(&mesh.chunks, tile, p);
            if by_id.get(&id).is_none_or(|s| s.under.is_none()) {
                let at = [p[0], p[1]];
                by_id.insert(id, Standing { model, at, under });
            }
        }
    }
    by_id
}

fn placed_on(mesh: &TileMesh) -> impl Iterator<Item = (String, &str, [f32; 3])> {
    let doodads = mesh.doodads.iter().map(|d| {
        (
            format!("doodad {}", d.unique_id),
            d.model.as_str(),
            d.position,
        )
    });
    let buildings = mesh.wmos.iter().map(|w| {
        (
            format!("building {}", w.unique_id),
            w.model.as_str(),
            w.position,
        )
    });
    doodads.chain(buildings)
}
