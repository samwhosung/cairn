use std::collections::HashMap;

use mpq::Chain;

use super::{Error, read_table, u32_at, u32_columns};

/// What a creature display's `CreatureSoundData` row says of its footfalls: the display's own
/// row, else its model's.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct CreatureVoices {
    display_to_sound: HashMap<u32, u32>,
    footstep_class: HashMap<u32, u32>,
}

impl CreatureVoices {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(
            chain,
            "DBFilesClient\\CreatureSoundData.dbc",
            &u32_columns::<30>(),
        )?;
        let footstep_class = rs
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 9).unwrap_or(0))))
            .collect();
        let rs = read_table(
            chain,
            "DBFilesClient\\CreatureModelData.dbc",
            &u32_columns::<16>(),
        )?;
        let model_to_sound: HashMap<u32, u32> = rs
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 13)?)))
            .filter(|&(_, sound)| sound != 0)
            .collect();
        let rs = read_table(
            chain,
            "DBFilesClient\\CreatureDisplayInfo.dbc",
            &u32_columns::<12>(),
        )?;
        let mut display_to_sound = HashMap::new();
        for r in rs.records() {
            let (Some(id), Some(sound), Some(model)) = (u32_at(r, 0), u32_at(r, 2), u32_at(r, 1))
            else {
                continue;
            };
            let sound = if sound == 0 {
                model_to_sound.get(&model).copied()
            } else {
                Some(sound)
            };
            if let Some(sound) = sound {
                display_to_sound.insert(id, sound);
            }
        }
        Ok(Self {
            display_to_sound,
            footstep_class,
        })
    }

    /// The display's footstep class; `None` without a sound row.
    pub fn footstep_class(&self, display: u32) -> Option<u32> {
        self.footstep_class
            .get(self.display_to_sound.get(&display)?)
            .copied()
    }
}
