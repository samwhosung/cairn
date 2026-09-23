use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;
use terrain::{Doodad, WmoInstance};

use crate::CurrentMap;
use crate::adt::AdtTile;
use crate::coords::{placement_rotation, wmo_doodad_local, wow_to_bevy};
use crate::source::{MPQ_SOURCE, m2_url, wmo_url};
use crate::stream::Streamer;
use crate::wdt::WdtIndex;
use crate::wmo::WmoModel;

/// The id the map-wide building of a map without terrain is placed under; no ADT uses it.
pub const GLOBAL_WMO_ID: u32 = u32::MAX;

/// What the map places where: every M2 doodad and WMO building of the tiles the stream holds,
/// each once, by the unique id the ADTs share across the tiles it overlaps. A placement stays while
/// any tile that names it is resident.
#[derive(Resource, Default)]
pub struct Placements {
    by_id: HashMap<u32, Placement>,
    tiles: HashMap<(u32, u32), Vec<u32>>,
    global_wmo: bool,
}

/// A doodad a WMO places inside itself: the M2, where it stands in the world, every group that
/// places it, and its index in the root's doodad list.
#[derive(Clone, Debug)]
pub struct PropPlacement {
    pub url: String,
    pub transform: Transform,
    pub groups: Arc<[u16]>,
    pub doodad: usize,
}

/// The doodads a building placed at `world` shows: set 0 and its own set, in root order, each
/// moved into the world. A doodad whose name does not resolve is left out.
pub fn prop_placements(wmo: &WmoModel, doodad_set: u16, world: &Transform) -> Vec<PropPlacement> {
    let mut ranges: Vec<(u32, u32)> = wmo
        .doodad_sets
        .first()
        .map(|s| (s.start, s.count))
        .into_iter()
        .collect();
    if doodad_set != 0
        && let Some(s) = wmo.doodad_sets.get(doodad_set as usize)
    {
        ranges.push((s.start, s.count));
    }
    let mut out = Vec::new();
    for (start, count) in ranges {
        for (di, d) in wmo
            .doodads
            .iter()
            .enumerate()
            .skip(start as usize)
            .take(count as usize)
        {
            if d.model.is_empty() {
                continue;
            }
            let local = wmo_doodad_local(d.position, d.orientation, d.scale);
            out.push(PropPlacement {
                url: m2_url(&d.model),
                transform: world.mul_transform(local),
                groups: wmo.doodad_groups.get(di).cloned().unwrap_or_default(),
                doodad: di,
            });
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct Placement {
    pub model: PlacedModel,
    /// Model space to world, in Bevy's axes.
    pub transform: Transform,
    refs: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlacedModel {
    /// An M2, by its `mpq://` URL.
    Doodad { url: String },
    /// A WMO root, by its `mpq://` URL, with the doodad set shown beside set 0 and its
    /// `WMOAreaTable` name set.
    Building {
        url: String,
        doodad_set: u16,
        name_set: u16,
    },
}

impl Placements {
    pub fn get(&self, id: u32) -> Option<&Placement> {
        self.by_id.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Placement)> {
        self.by_id.iter().map(|(&id, p)| (id, p))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    fn add(&mut self, id: u32, place: impl FnOnce() -> Placement) {
        self.by_id.entry(id).or_insert_with(place).refs += 1;
    }

    fn release(&mut self, id: u32) {
        if let Some(p) = self.by_id.get_mut(&id) {
            p.refs -= 1;
            if p.refs == 0 {
                self.by_id.remove(&id);
            }
        }
    }

    fn register_tile(&mut self, key: (u32, u32), adt: &AdtTile) {
        let mut ids = Vec::with_capacity(adt.doodads.len() + adt.wmos.len());
        for d in &adt.doodads {
            self.add(d.unique_id, || doodad(d));
            ids.push(d.unique_id);
        }
        for w in &adt.wmos {
            self.add(w.unique_id, || building(w));
            ids.push(w.unique_id);
        }
        self.tiles.insert(key, ids);
    }
}

fn doodad(d: &Doodad) -> Placement {
    Placement {
        model: PlacedModel::Doodad {
            url: m2_url(&d.model),
        },
        transform: Transform {
            translation: wow_to_bevy(d.position),
            rotation: placement_rotation(d.rotation),
            scale: Vec3::splat(d.scale),
        },
        refs: 0,
    }
}

fn building(w: &WmoInstance) -> Placement {
    Placement {
        model: PlacedModel::Building {
            url: wmo_url(&w.model),
            doodad_set: w.doodad_set,
            name_set: w.name_set,
        },
        transform: Transform {
            translation: wow_to_bevy(w.position),
            rotation: placement_rotation(w.rotation),
            scale: Vec3::ONE,
        },
        refs: 0,
    }
}

pub(crate) fn track_placements(
    server: Res<'_, AssetServer>,
    map: Res<'_, CurrentMap>,
    streamer: Res<'_, Streamer>,
    adts: Res<'_, Assets<AdtTile>>,
    wdts: Res<'_, Assets<WdtIndex>>,
    mut wdt: Local<'_, Option<Handle<WdtIndex>>>,
    mut placements: ResMut<'_, Placements>,
) {
    let dir = map.directory.to_ascii_lowercase();
    let wdt = wdt
        .get_or_insert_with(|| server.load(format!("{MPQ_SOURCE}://world/maps/{dir}/{dir}.wdt")));
    if !placements.global_wmo
        && let Some(g) = wdts.get(&*wdt).and_then(WdtIndex::global_wmo)
    {
        let w = WmoInstance {
            model: g.model.clone(),
            position: g.position,
            rotation: g.rotation,
            unique_id: GLOBAL_WMO_ID,
            doodad_set: g.doodad_set,
            name_set: g.name_set,
        };
        placements.add(GLOBAL_WMO_ID, || building(&w));
        placements.global_wmo = true;
    }
    let arrived: HashSet<(u32, u32)> = streamer.arrived().map(|(key, _)| key).collect();
    let gone: Vec<(u32, u32)> = placements
        .tiles
        .keys()
        .filter(|key| !arrived.contains(key))
        .copied()
        .collect();
    for key in gone {
        let ids = placements.tiles.remove(&key).unwrap_or_default();
        for id in ids {
            placements.release(id);
        }
    }
    for (key, handle) in streamer.arrived() {
        if placements.tiles.contains_key(&key) {
            continue;
        }
        if let Some(adt) = adts.get(handle) {
            placements.register_tile(key, adt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(ids: &[u32]) -> Vec<Doodad> {
        ids.iter()
            .map(|&unique_id| Doodad {
                model: "World\\Azeroth\\Elwynn\\PassiveDoodads\\Tree.mdx".into(),
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 90.0, 0.0],
                scale: 1.5,
                unique_id,
            })
            .collect()
    }

    #[test]
    fn a_doodad_two_tiles_name_stays_until_both_go() {
        let mut p = Placements::default();
        for (key, ids) in [((1, 1), [7u32, 8]), ((1, 2), [8, 9])] {
            for d in tile(&ids) {
                p.add(d.unique_id, || doodad(&d));
            }
            p.tiles.insert(key, ids.to_vec());
        }
        assert_eq!(p.len(), 3);
        for id in p.tiles.remove(&(1, 1)).unwrap_or_default() {
            p.release(id);
        }
        assert!(p.get(7).is_none());
        let shared = p.get(8).expect("the second tile still names it");
        assert_eq!(
            shared.model,
            PlacedModel::Doodad {
                url: "mpq://world/azeroth/elwynn/passivedoodads/tree.m2".into()
            }
        );
        assert_eq!(shared.transform.translation, wow_to_bevy([1.0, 2.0, 3.0]));
        assert_eq!(shared.transform.scale, Vec3::splat(1.5));
    }
}
