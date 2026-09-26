mod filed;

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;
use terrain::{Doodad, WmoInstance};

use crate::CurrentMap;
use crate::adt::AdtTile;
use crate::coords::wmo_doodad_local;
use crate::source::{MPQ_SOURCE, m2_url};
use crate::stream::Streamer;
use crate::wdt::WdtIndex;
use crate::wmo::WmoModel;
pub use filed::{Filed, PlacementEdits};

/// The id the map-wide building of a map without terrain is placed under; no ADT uses it.
pub const GLOBAL_WMO_ID: u32 = u32::MAX;

/// What the map places where: every M2 doodad and WMO building of the tiles the stream holds,
/// each once, by the unique id the ADTs share across the tiles it overlaps, with the
/// [`PlacementEdits`] made at run time over them. A placement stays while any tile that names it
/// is resident; an edited one while its edit holds.
#[derive(Resource, Default)]
pub struct Placements {
    by_id: BTreeMap<u32, Placement>,
    tiles: BTreeMap<(u32, u32), Vec<u32>>,
    global_wmo: bool,
    edited: BTreeMap<u32, Option<Placement>>,
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
    /// As the map's files or an edit hold it; `None` for one put there by [`Placements::place`].
    pub filed: Option<Filed>,
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
        match self.edited.get(&id) {
            Some(edited) => edited.as_ref(),
            None => self.by_id.get(&id),
        }
    }

    /// In the order of their unique ids.
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Placement)> {
        let mut held = self
            .by_id
            .iter()
            .filter(|(id, _)| !self.edited.contains_key(id))
            .map(|(&id, p)| (id, p))
            .peekable();
        let mut edited = self
            .edited
            .iter()
            .filter_map(|(&id, p)| Some((id, p.as_ref()?)))
            .peekable();
        std::iter::from_fn(move || match (held.peek(), edited.peek()) {
            (Some(h), Some(e)) if h.0 < e.0 => held.next(),
            (Some(_), None) => held.next(),
            _ => edited.next(),
        })
    }

    /// Whether an edit stands over what the tiles put under `id`.
    pub fn is_edited(&self, id: u32) -> bool {
        self.edited.contains_key(&id)
    }

    pub fn len(&self) -> usize {
        self.iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Places `model` at `transform` under `id` until [`Self::lift`] takes it away. An `id` a tile
    /// or an earlier place still holds keeps what it has, and only counts one more holder.
    pub fn place(&mut self, id: u32, model: PlacedModel, transform: Transform) {
        self.add(id, || Placement {
            model,
            transform,
            filed: None,
            refs: 0,
        });
    }

    /// Undoes one [`Self::place`] under `id`; what is there goes once nothing holds it.
    pub fn lift(&mut self, id: u32) {
        self.release(id);
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
    Filed::Doodad(d.clone()).placement()
}

fn building(w: &WmoInstance) -> Placement {
    Filed::Building(w.clone()).placement()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn track_placements(
    server: Res<'_, AssetServer>,
    map: Res<'_, CurrentMap>,
    streamer: Res<'_, Streamer>,
    adts: Res<'_, Assets<AdtTile>>,
    wdts: Res<'_, Assets<WdtIndex>>,
    edits: Res<'_, PlacementEdits>,
    mut wdt: Local<'_, Option<Handle<WdtIndex>>>,
    mut placements: ResMut<'_, Placements>,
) {
    if edits.is_changed() {
        placements.edited = edits
            .iter()
            .map(|(id, filed)| (id, filed.map(Filed::placement)))
            .collect();
    }
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
    use crate::coords::wow_to_bevy;

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

    #[test]
    fn a_model_placed_by_hand_stays_until_it_is_lifted() {
        let mut p = Placements::default();
        let lamp = PlacedModel::Doodad {
            url: m2_url("World\\Lamp.mdx"),
        };
        let at = Transform::from_xyz(1.0, 2.0, 3.0);
        p.place(4, lamp.clone(), at);
        assert_eq!(p.get(4).map(|q| (&q.model, q.transform)), Some((&lamp, at)));
        p.lift(4);
        assert!(p.is_empty());
        p.lift(4);
        assert!(p.is_empty(), "lifting what is gone does nothing");
    }

    #[test]
    fn an_edit_stands_over_the_tiles_in_id_order_until_it_changes_again() {
        let mut p = Placements::default();
        for d in tile(&[3, 5, 8]) {
            p.add(d.unique_id, || doodad(&d));
        }
        let mut edits = PlacementEdits::default();
        let moved = Filed::Doodad(tile(&[5]).remove(0)).stood([4.0, 2.0, 3.0], [0.0; 3], 1.0);
        edits.place(moved.clone());
        edits.place(moved.clone().with_id(6));
        edits.remove(8);
        p.edited = edits
            .iter()
            .map(|(id, filed)| (id, filed.map(Filed::placement)))
            .collect();
        let ids: Vec<u32> = p.iter().map(|(id, _)| id).collect();
        assert_eq!(ids, [3, 5, 6], "the moved, the added, and not the removed");
        assert_eq!(p.get(5).and_then(|q| q.filed.as_ref()), Some(&moved));
        assert_eq!(p.get(5).map(|q| q.transform), Some(moved.transform()));
        assert!(p.get(8).is_none());
        assert_eq!(p.len(), 3);
    }
}
