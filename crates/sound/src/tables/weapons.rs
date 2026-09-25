use std::collections::HashMap;

use mpq::Chain;

use super::{Error, read_table, u32_at, u32_columns};

/// What a melee weapon sounds: its impact kits by weapon subclass and metal, off
/// `WeaponImpactSounds.dbc`, and the connecting swing's kits by weight, off
/// `WeaponSwingSounds2.dbc`.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct WeaponSounds {
    impacts: HashMap<(u32, bool), Impact>,
    swings: [u32; SWING_WEIGHTS * 2],
}

/// One weapon's impact kits, a normal and a critical one for each [`impact_slot`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Impact {
    pub normal: [u32; 10],
    pub critical: [u32; 10],
}

/// What a weapon strikes, as its row's columns order it.
pub mod impact_slot {
    pub const FLESH: usize = 0;
    pub const WOOD: usize = 7;
    pub const STONE: usize = 8;
    pub const ETHEREAL: usize = 9;
}

/// How heavy a weapon swings; the client reads no heavier weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwingWeight {
    Light,
    Medium,
    Heavy,
}

const SWING_WEIGHTS: usize = 3;

impl WeaponSounds {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(
            chain,
            "DBFilesClient\\WeaponImpactSounds.dbc",
            &u32_columns::<23>(),
        )?;
        let mut impacts = HashMap::new();
        for r in rs.records() {
            let (Some(subclass), Some(metal)) = (u32_at(r, 1), u32_at(r, 2)) else {
                continue;
            };
            let col = |i| u32_at(r, i).unwrap_or(0);
            let impact = Impact {
                normal: std::array::from_fn(|slot| col(3 + slot)),
                critical: std::array::from_fn(|slot| col(13 + slot)),
            };
            impacts.insert((subclass, metal != 0), impact);
        }
        let rs = read_table(
            chain,
            "DBFilesClient\\WeaponSwingSounds2.dbc",
            &u32_columns::<4>(),
        )?;
        let mut swings = [0; SWING_WEIGHTS * 2];
        for r in rs.records() {
            let col = |i| u32_at(r, i).unwrap_or(0);
            let (weight, critical) = (col(1) as usize, col(2) as usize);
            if weight < SWING_WEIGHTS && critical < 2 {
                swings[weight * 2 + critical] = col(3);
            }
        }
        Ok(Self { impacts, swings })
    }

    /// The subclass's row in its metal, else in the other, as some have only one.
    pub fn impact(&self, subclass: u32, metal: bool) -> Option<&Impact> {
        self.impacts
            .get(&(subclass, metal))
            .or_else(|| self.impacts.get(&(subclass, !metal)))
    }

    /// The connecting swing's kit.
    pub fn swing(&self, weight: SwingWeight, critical: bool) -> Option<u32> {
        let kit = self.swings[weight as usize * 2 + usize::from(critical)];
        (kit != 0).then_some(kit)
    }

    pub fn len(&self) -> usize {
        self.impacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.impacts.is_empty()
    }
}
