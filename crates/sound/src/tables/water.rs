use std::collections::HashMap;

use mpq::Chain;

use super::{Error, read_table, u32_at, u32_columns};

/// `SoundWaterType`: the loop a liquid's sound nibble plays, keyed by `(class, speed)`.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct WaterSounds {
    by_key: HashMap<(u32, u32), u32>,
}

impl WaterSounds {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(
            chain,
            "DBFilesClient\\SoundWaterType.dbc",
            &u32_columns::<4>(),
        )?;
        let mut by_key = HashMap::new();
        for r in rs.records() {
            let (Some(class), Some(speed), Some(kit)) = (u32_at(r, 1), u32_at(r, 2), u32_at(r, 3))
            else {
                continue;
            };
            if kit != 0 {
                by_key.insert((class, speed), kit);
            }
        }
        Ok(Self { by_key })
    }

    /// The loop for a nibble: class `n & 3`, speed `n & 0xc`.
    pub fn kit_for_nibble(&self, nibble: u8) -> Option<u32> {
        let n = u32::from(nibble & 0xf);
        self.by_key.get(&(n & 3, n & 0xc)).copied()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}
