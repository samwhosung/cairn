use std::collections::HashMap;

use dbc::FieldType;
use mpq::Chain;

use super::{Error, read_table, u32_at, u32_columns};

/// The footstep chain: a ground effect's `TerrainType`, the terrain's sound class, and the
/// `(footstep class, sound class)` lookup to a dry and a splash kit.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct Footsteps {
    effect_terrain: HashMap<u32, u32>,
    terrain_sound: HashMap<u32, u32>,
    lookup: HashMap<(u32, u32), (u32, u32)>,
}

impl Footsteps {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let mut cat = Self::default();
        let rs = read_table(
            chain,
            "DBFilesClient\\GroundEffectTexture.dbc",
            &u32_columns::<7>(),
        )?;
        for r in rs.records() {
            if let (Some(id), Some(terrain)) = (u32_at(r, 0), u32_at(r, 6)) {
                cat.effect_terrain.insert(id, terrain);
            }
        }
        let mut columns = u32_columns::<6>().to_vec();
        columns[1] = ("Name", FieldType::String);
        let rs = read_table(chain, "DBFilesClient\\TerrainType.dbc", &columns)?;
        for r in rs.records() {
            if let (Some(id), Some(class)) = (u32_at(r, 0), u32_at(r, 4)) {
                cat.terrain_sound.insert(id, class);
            }
        }
        let rs = read_table(
            chain,
            "DBFilesClient\\FootstepTerrainLookup.dbc",
            &u32_columns::<5>(),
        )?;
        for r in rs.records() {
            let (Some(class), Some(sound_class)) = (u32_at(r, 1), u32_at(r, 2)) else {
                continue;
            };
            cat.lookup.insert(
                (class, sound_class),
                (u32_at(r, 3).unwrap_or(0), u32_at(r, 4).unwrap_or(0)),
            );
        }
        Ok(cat)
    }

    /// The `TerrainType` a ground effect names.
    pub fn terrain_of(&self, effect: u32) -> Option<u32> {
        self.effect_terrain.get(&effect).copied()
    }

    /// The `(dry, splash)` kits a footstep class makes on a terrain type; `None` is silent.
    pub fn resolve_terrain(&self, footstep_class: u32, terrain: u32) -> Option<(u32, u32)> {
        let sound_class = self.terrain_sound.get(&terrain).copied()?;
        self.lookup.get(&(footstep_class, sound_class)).copied()
    }

    /// Every terrain type the table knows, and every footstep class the lookup names.
    pub fn domain(&self) -> (Vec<u32>, Vec<u32>) {
        let mut terrains: Vec<u32> = self.terrain_sound.keys().copied().collect();
        let mut classes: Vec<u32> = self.lookup.keys().map(|&(c, _)| c).collect();
        terrains.sort_unstable();
        classes.sort_unstable();
        classes.dedup();
        (terrains, classes)
    }

    pub fn len(&self) -> usize {
        self.lookup.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lookup.is_empty()
    }
}
