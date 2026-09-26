use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use atlas::{Areas, Doodads};
use mpq::Chain;
use rayon::prelude::*;

use crate::scan::{self, ChunkSummary, MapTiles, TileSummary};
use crate::{
    Example, Ground, Model, Place, Scales, Survey, Tally, TileSpan, WetCells, Zone, key, spelling,
};

const MIN_BESIDE_TEXELS: f32 = 41.0;
const EXAMPLES: usize = 5;
const DEFAULT_DOODAD_SET: u16 = 0;
const WHOLE_MAP_BUILDING_ID: u32 = u32::MAX;

type ZoneId = (usize, u32);

#[derive(Default)]
struct ZoneAcc {
    chunks: u32,
    tiles: Option<TileSpan>,
    wet_cells: WetCells,
    places: BTreeMap<u32, u32>,
    grounds: BTreeMap<usize, f64>,
    models: BTreeMap<usize, (u32, u32)>,
    doodads: Doodads,
    buildings: u32,
}

#[derive(Default)]
struct GroundAcc {
    spellings: BTreeMap<String, u32>,
    texels: f64,
    chunks: u32,
    zones: BTreeMap<ZoneId, f64>,
    beside: BTreeMap<usize, f64>,
}

#[derive(Default)]
struct ModelAcc {
    spellings: BTreeMap<String, u32>,
    on_ground: u32,
    in_buildings: u32,
    zones: BTreeMap<ZoneId, (u32, u32)>,
    places: BTreeMap<(ZoneId, u32), u32>,
    scales: Vec<f32>,
    examples: Vec<(u32, u32, Example)>,
}

struct Placed<'a> {
    map: usize,
    area: Option<u32>,
    unique_id: u32,
    model: &'a str,
    position: [f32; 3],
    heading: f32,
    scale: f32,
    doodad_set: u16,
}

struct WmoRoot {
    bounds: Option<[[f32; 3]; 2]>,
    doodad_sets: Vec<Vec<SetDoodad>>,
}

struct SetDoodad {
    model: String,
    scale: f32,
}

pub(crate) fn survey(chain: &Chain) -> Result<Survey, String> {
    let areas = Areas::load(chain)?;
    let maps = scan::maps(chain)?;
    let tiles = scan::tiles(chain, &maps);
    let mut gathering = Gathering {
        areas: &areas,
        maps: &maps,
        zones: BTreeMap::new(),
        models: BTreeMap::new(),
        buildings: BTreeSet::new(),
    };
    let grounds = gathering.paint(&tiles);
    let doodads = each_once(&tiles, |t| {
        t.doodads.iter().map(|d| {
            let p = &d.placed;
            (
                d.area_here,
                p.unique_id,
                p.model.as_str(),
                p.position,
                p.rotation[1],
                p.scale,
                0,
            )
        })
    });
    let wmos = buildings(&tiles, &maps, &areas);
    let root_keys: BTreeMap<String, &str> = wmos.iter().map(|w| (key(w.model), w.model)).collect();
    let roots: BTreeMap<String, WmoRoot> = root_keys
        .par_iter()
        .map(|(k, path)| (k.clone(), wmo_root(chain, path)))
        .collect();
    gathering.place_doodads(&doodads);
    gathering.place_buildings(&wmos, &roots);
    Ok(gathering.finish(chain, &roots, &grounds))
}

fn buildings<'a>(tiles: &'a [TileSummary], maps: &'a [MapTiles], areas: &Areas) -> Vec<Placed<'a>> {
    let mut wmos = each_once(tiles, |t| {
        t.wmos.iter().map(|w| {
            let p = &w.placed;
            let set = p.doodad_set;
            (
                w.area_here,
                p.unique_id,
                p.model.as_str(),
                p.position,
                p.rotation[1],
                1.0,
                set,
            )
        })
    });
    for (map, m) in maps.iter().enumerate() {
        if let Some(g) = &m.global_wmo {
            wmos.push(Placed {
                map,
                area: areas.zones_on(m.id).first().copied(),
                unique_id: WHOLE_MAP_BUILDING_ID,
                model: &g.model,
                position: g.position,
                heading: g.rotation[1],
                scale: 1.0,
                doodad_set: g.doodad_set,
            });
        }
    }
    wmos
}

struct Gathering<'a> {
    areas: &'a Areas,
    maps: &'a [MapTiles],
    zones: BTreeMap<ZoneId, ZoneAcc>,
    models: BTreeMap<String, ModelAcc>,
    buildings: BTreeSet<String>,
}

struct Grounds {
    keys: BTreeSet<String>,
    accs: Vec<GroundAcc>,
}

impl Gathering<'_> {
    fn zone_of(&self, map: usize, area: Option<u32>) -> ZoneId {
        (map, area.and_then(|a| self.areas.top_zone(a)).unwrap_or(0))
    }

    fn paint(&mut self, tiles: &[TileSummary]) -> Grounds {
        let keys: BTreeSet<String> = tiles
            .iter()
            .flat_map(|t| &t.chunks)
            .flat_map(|c| c.paint.iter().map(|p| key(&p.texture)))
            .collect();
        let index: BTreeMap<&str, usize> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| (k.as_str(), i))
            .collect();
        let mut accs: Vec<GroundAcc> = keys.iter().map(|_| GroundAcc::default()).collect();
        for tile in tiles {
            for chunk in &tile.chunks {
                let zone = self.zone_of(tile.map, Some(chunk.area));
                let acc = self.zones.entry(zone).or_default();
                paint(chunk, tile, zone, acc, &index, &mut accs);
            }
        }
        Grounds { keys, accs }
    }

    fn place_doodads(&mut self, doodads: &[Placed<'_>]) {
        for d in doodads {
            let zone = self.zone_of(d.map, d.area);
            self.zones
                .entry(zone)
                .or_default()
                .doodads
                .count(atlas::kind(d.model));
            let m = self.models.entry(key(d.model)).or_default();
            m.place(d, self.maps, zone, true, None);
        }
    }

    fn place_buildings(&mut self, wmos: &[Placed<'_>], roots: &BTreeMap<String, WmoRoot>) {
        for w in wmos {
            let zone = self.zone_of(w.map, w.area);
            self.zones.entry(zone).or_default().buildings += 1;
            let k = key(w.model);
            self.buildings.insert(k.clone());
            let m = self.models.entry(k.clone()).or_default();
            m.place(w, self.maps, zone, true, None);
            let Some(root) = roots.get(&k) else { continue };
            let own = (w.doodad_set != DEFAULT_DOODAD_SET).then_some(w.doodad_set);
            for set in std::iter::once(DEFAULT_DOODAD_SET).chain(own) {
                for d in root.doodad_sets.get(usize::from(set)).into_iter().flatten() {
                    let inside = Placed {
                        scale: d.scale,
                        model: &d.model,
                        ..*w
                    };
                    let m = self.models.entry(key(&d.model)).or_default();
                    m.place(&inside, self.maps, zone, false, Some(w.model));
                }
            }
        }
    }

    fn finish(
        mut self,
        chain: &Chain,
        roots: &BTreeMap<String, WmoRoot>,
        grounds: &Grounds,
    ) -> Survey {
        let model_keys: Vec<&String> = self.models.keys().collect();
        let shapes: Vec<Option<Shape>> = model_keys
            .par_iter()
            .map(|k| match roots.get(*k) {
                Some(root) => root.bounds.map(|bounds| Shape { bounds, mesh: true }),
                None => doodad_shape(chain, &spelling(&self.models[*k].spellings)),
            })
            .collect();
        for (i, m) in self.models.values().enumerate() {
            for (&zone, &placed) in &m.zones {
                self.zones.entry(zone).or_default().models.insert(i, placed);
            }
        }
        let zone_ids: Vec<ZoneId> = self.zones.keys().copied().collect();
        let zone_index: BTreeMap<ZoneId, usize> =
            zone_ids.iter().enumerate().map(|(i, z)| (*z, i)).collect();
        let names: Vec<String> = zone_ids
            .iter()
            .map(|&(map, area)| match area {
                0 => format!("{} (no zone)", self.maps[map].directory),
                _ => zone_name(self.areas, area),
            })
            .collect();
        let keys = zone_keys(&zone_ids, &names, self.maps);
        let zones: Vec<Zone> = std::mem::take(&mut self.zones)
            .into_iter()
            .zip(names.iter().zip(keys))
            .map(|(((map, area), acc), (name, key))| {
                finish_zone(acc, area, name, key, &self.maps[map], self.areas)
            })
            .collect();
        let grounds: Vec<Ground> = grounds
            .accs
            .iter()
            .zip(&grounds.keys)
            .map(|(acc, k)| finish_ground(acc, k, &zone_index))
            .collect();
        let models: Vec<Model> = std::mem::take(&mut self.models)
            .into_iter()
            .zip(shapes)
            .map(|((k, acc), shape)| {
                let building = self.buildings.contains(&k);
                finish_model(acc, k, building, shape, &zone_index, self.areas)
            })
            .collect();
        Survey {
            zones,
            grounds,
            models,
        }
    }
}

fn paint(
    chunk: &ChunkSummary,
    tile: &TileSummary,
    zone: ZoneId,
    acc: &mut ZoneAcc,
    index: &BTreeMap<&str, usize>,
    grounds: &mut [GroundAcc],
) {
    acc.chunks += 1;
    let (x, y) = tile.at;
    acc.tiles = Some(match acc.tiles {
        None => TileSpan {
            x0: x,
            x1: x,
            y0: y,
            y1: y,
        },
        Some(t) => TileSpan {
            x0: t.x0.min(x),
            x1: t.x1.max(x),
            y0: t.y0.min(y),
            y1: t.y1.max(y),
        },
    });
    let (sum, wet) = (&mut acc.wet_cells, chunk.wet_cells);
    sum.water += wet.water;
    sum.ocean += wet.ocean;
    sum.magma += wet.magma;
    sum.slime += wet.slime;
    *acc.places.entry(chunk.area).or_default() += 1;
    let mut shown: BTreeMap<usize, (f32, &str)> = BTreeMap::new();
    for p in &chunk.paint {
        let e = shown
            .entry(index[key(&p.texture).as_str()])
            .or_insert((0.0, &p.texture));
        e.0 += p.texels;
    }
    for (&g, &(texels, path)) in &shown {
        let acc_g = &mut grounds[g];
        *acc_g.spellings.entry(path.to_owned()).or_default() += 1;
        acc_g.texels += f64::from(texels);
        acc_g.chunks += 1;
        *acc_g.zones.entry(zone).or_default() += f64::from(texels);
        *acc.grounds.entry(g).or_default() += f64::from(texels);
        if texels < MIN_BESIDE_TEXELS {
            continue;
        }
        for (&other, &(t, _)) in &shown {
            if other != g && t >= MIN_BESIDE_TEXELS {
                *grounds[g].beside.entry(other).or_default() += f64::from(texels);
            }
        }
    }
}

type Listing<'a> = (Option<u32>, u32, &'a str, [f32; 3], f32, f32, u16);

/// A tile lists every placement that overlaps it, and an id is unique on its map.
fn each_once<'a, I>(
    tiles: &'a [TileSummary],
    listed: impl Fn(&'a TileSummary) -> I,
) -> Vec<Placed<'a>>
where
    I: Iterator<Item = Listing<'a>>,
{
    let mut by_id: BTreeMap<(usize, u32), Placed<'a>> = BTreeMap::new();
    for tile in tiles {
        for (area, unique_id, model, position, heading, scale, doodad_set) in listed(tile) {
            let placed = Placed {
                map: tile.map,
                area,
                unique_id,
                model,
                position,
                heading,
                scale,
                doodad_set,
            };
            match by_id.get(&(tile.map, unique_id)) {
                Some(p) if p.area.is_some() || area.is_none() => {}
                _ => {
                    by_id.insert((tile.map, unique_id), placed);
                }
            }
        }
    }
    by_id.into_values().collect()
}

impl ModelAcc {
    fn place(
        &mut self,
        p: &Placed<'_>,
        maps: &[MapTiles],
        zone: ZoneId,
        on_ground: bool,
        building: Option<&str>,
    ) {
        *self.spellings.entry(p.model.to_owned()).or_default() += 1;
        let (g, i) = self.zones.entry(zone).or_default();
        if on_ground {
            self.on_ground += 1;
            *g += 1;
        } else {
            self.in_buildings += 1;
            *i += 1;
        }
        *self.places.entry((zone, p.area.unwrap_or(0))).or_default() += 1;
        self.scales.push(p.scale);
        let example = Example {
            map_directory: maps[p.map].directory.clone(),
            position: p.position,
            heading: p.heading,
            scale: p.scale,
            building: building.map(str::to_owned),
        };
        let rank = (maps[p.map].id, p.unique_id);
        if self.examples.len() < EXAMPLES || self.examples.iter().any(|e| (e.0, e.1) > rank) {
            self.examples.push((rank.0, rank.1, example));
            self.examples.sort_by(|a, b| {
                (a.0, a.1, a.2.building.is_some())
                    .cmp(&(b.0, b.1, b.2.building.is_some()))
                    .then(a.2.position[0].total_cmp(&b.2.position[0]))
                    .then(a.2.position[1].total_cmp(&b.2.position[1]))
            });
            self.examples.truncate(EXAMPLES);
        }
    }
}

/// The box the model at `path` fills, as [`Model::bounds`]: a building's groups, or a doodad's
/// vertices at rest.
pub fn model_bounds(chain: &Chain, path: &str) -> Option<[[f32; 3]; 2]> {
    if path.to_ascii_lowercase().ends_with(".wmo") {
        wmo_root(chain, path).bounds
    } else {
        doodad_shape(chain, path).map(|s| s.bounds)
    }
}

struct Shape {
    bounds: [[f32; 3]; 2],
    mesh: bool,
}

fn wmo_root(chain: &Chain, path: &str) -> WmoRoot {
    let none = || WmoRoot {
        bounds: None,
        doodad_sets: Vec::new(),
    };
    let Ok(bytes) = chain.read(path) else {
        return none();
    };
    let Ok(root) = model::parse_wmo_root(&bytes) else {
        return none();
    };
    let bounds = root
        .group_infos()
        .iter()
        .fold(None, |b: Option<[[f32; 3]; 2]>, g| {
            Some(match b {
                None => [g.bbox_min, g.bbox_max],
                Some([lo, hi]) => [min3(lo, g.bbox_min), max3(hi, g.bbox_max)],
            })
        });
    let doodad_sets = root
        .doodad_sets()
        .iter()
        .map(|s| {
            root.doodads()
                .iter()
                .skip(s.start as usize)
                .take(s.count as usize)
                .filter(|d| !d.model.is_empty())
                .map(|d| SetDoodad {
                    model: d.model.clone(),
                    scale: d.scale,
                })
                .collect()
        })
        .collect();
    WmoRoot {
        bounds,
        doodad_sets,
    }
}

fn doodad_shape(chain: &Chain, path: &str) -> Option<Shape> {
    let m2 = match path.to_ascii_lowercase().rsplit_once('.') {
        Some((stem, "mdx" | "mdl")) => format!("{stem}.m2"),
        _ => path.to_owned(),
    };
    let bytes = chain.read(&m2).ok()?;
    let parsed = m2::parse_m2(&mut Cursor::new(bytes.as_slice())).ok()?;
    let model = parsed.model();
    let from_vertices = model
        .vertices
        .iter()
        .fold(None, |b: Option<[[f32; 3]; 2]>, v| {
            let p = [v.position.x, v.position.y, v.position.z];
            Some(match b {
                None => [p, p],
                Some([lo, hi]) => [min3(lo, p), max3(hi, p)],
            })
        });
    Some(match from_vertices {
        Some(bounds) => Shape { bounds, mesh: true },
        None => Shape {
            bounds: [model.bounds.bounding_box_min, model.bounds.bounding_box_max],
            mesh: false,
        },
    })
}

fn min3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])]
}

fn max3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])]
}

fn zone_name(areas: &Areas, area: u32) -> String {
    areas
        .get(area)
        .map_or_else(|| "(no zone)".to_owned(), |a| a.name.clone())
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[derive(Clone, Copy)]
enum KeySuffix {
    MapDirectory,
    MapId,
    Area,
}

fn zone_keys(ids: &[ZoneId], names: &[String], maps: &[MapTiles]) -> Vec<String> {
    let unique = |keys: &[String], i: usize| keys.iter().filter(|k| **k == keys[i]).count() == 1;
    let mut keys: Vec<String> = names.iter().map(|n| slug(n)).collect();
    for suffix in [KeySuffix::MapDirectory, KeySuffix::MapId, KeySuffix::Area] {
        let shared: Vec<usize> = (0..keys.len()).filter(|&i| !unique(&keys, i)).collect();
        for i in shared {
            let (map, area) = ids[i];
            let more = match suffix {
                KeySuffix::MapDirectory => slug(&maps[map].directory),
                KeySuffix::MapId => maps[map].id.to_string(),
                KeySuffix::Area => area.to_string(),
            };
            keys[i] = format!("{}-{more}", keys[i]);
        }
    }
    keys
}

fn finish_zone(
    acc: ZoneAcc,
    area: u32,
    name: &str,
    key: String,
    map: &MapTiles,
    areas: &Areas,
) -> Zone {
    let mut places: Vec<(String, u32)> = acc
        .places
        .iter()
        .map(|(&a, &n)| (zone_name(areas, a), n))
        .collect();
    places.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut grounds: Vec<(usize, f64)> = acc.grounds.into_iter().collect();
    grounds.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut models: Vec<Tally> = acc
        .models
        .into_iter()
        .map(|(index, (on_ground, in_buildings))| Tally {
            index,
            on_ground,
            in_buildings,
        })
        .collect();
    models.sort_by(|a, b| b.placed().cmp(&a.placed()).then(a.index.cmp(&b.index)));
    Zone {
        area,
        name: name.to_owned(),
        map: map.id,
        map_directory: map.directory.clone(),
        key,
        chunks: acc.chunks,
        tiles: acc.tiles,
        wet_cells: acc.wet_cells,
        places,
        grounds,
        models,
        doodads: acc.doodads,
        buildings: acc.buildings,
    }
}

fn finish_ground(acc: &GroundAcc, k: &str, zone_index: &BTreeMap<ZoneId, usize>) -> Ground {
    let path = spelling(&acc.spellings);
    let mut zones: Vec<(usize, f64)> = acc.zones.iter().map(|(z, t)| (zone_index[z], *t)).collect();
    zones.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut beside: Vec<(usize, f64)> = acc
        .beside
        .iter()
        .map(|(&g, &t)| (g, t / acc.texels.max(1.0)))
        .collect();
    beside.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Ground {
        kind: crate::words::ground_kind(&path),
        path,
        key: k.to_owned(),
        texels: acc.texels,
        chunks: acc.chunks,
        zones,
        beside,
    }
}

fn finish_model(
    mut acc: ModelAcc,
    k: String,
    building: bool,
    shape: Option<Shape>,
    zone_index: &BTreeMap<ZoneId, usize>,
    areas: &Areas,
) -> Model {
    let path = spelling(&acc.spellings);
    let kind = if building {
        "building"
    } else {
        atlas::kind(&path).name()
    };
    let mut zones: Vec<Tally> = acc
        .zones
        .iter()
        .map(|(z, &(on_ground, in_buildings))| Tally {
            index: zone_index[z],
            on_ground,
            in_buildings,
        })
        .collect();
    zones.sort_by(|a, b| b.placed().cmp(&a.placed()).then(a.index.cmp(&b.index)));
    let mut places: Vec<Place> = acc
        .places
        .iter()
        .map(|(&(z, area), &placements)| Place {
            zone: zone_index[&z],
            area: zone_name(areas, area),
            placements,
        })
        .collect();
    let rank: BTreeMap<usize, usize> = zones
        .iter()
        .enumerate()
        .map(|(r, z)| (z.index, r))
        .collect();
    places.sort_by(|a, b| {
        rank[&a.zone]
            .cmp(&rank[&b.zone])
            .then(b.placements.cmp(&a.placements))
            .then(a.area.cmp(&b.area))
    });
    acc.scales.sort_by(f32::total_cmp);
    let at = |percentile: usize| acc.scales[(acc.scales.len() - 1) * percentile / 100];
    let scales = (!acc.scales.is_empty()).then(|| Scales {
        least: at(0),
        p10: at(10),
        p50: at(50),
        p90: at(90),
        most: at(100),
    });
    Model {
        path,
        key: k,
        building,
        kind,
        bounds: shape.as_ref().map(|s| s.bounds),
        mesh: shape.is_some_and(|s| s.mesh),
        on_ground: acc.on_ground,
        in_buildings: acc.in_buildings,
        zones,
        places,
        scales,
        examples: acc.examples.into_iter().map(|e| e.2).collect(),
    }
}

#[cfg(test)]
mod tests;
