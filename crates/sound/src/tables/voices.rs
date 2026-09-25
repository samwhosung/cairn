use std::collections::HashMap;

use mpq::Chain;

use super::{Error, read_table, u32_at, u32_columns};

/// What a creature display's `CreatureSoundData` row says: the display's own row, else its
/// model's.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct CreatureVoices {
    display_to_sound: HashMap<u32, u32>,
    rows: HashMap<u32, Voice>,
}

/// One `CreatureSoundData` row's fight and footfall: each a `SoundEntries` kit, 0 for none,
/// unless named otherwise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    /// The attacker's grunt: a swing's, a critical one's.
    pub exertion: [u32; 2],
    /// The cry of the one hit: a hit's, a critical one's, a crushing one's.
    pub injury: [u32; 3],
    pub death: u32,
    /// A class of `FootstepTerrainLookup`, not a kit.
    pub footstep_class: u32,
    /// What a clip's `$AH0`..`$AH3` key sounds in place of a weapon's impact.
    pub custom_attack: [u32; 4],
    /// What a weapon strikes in the body: 0 flesh, 1 stone, 2 wood, 3 ethereal.
    pub impact_type: u32,
}

impl CreatureVoices {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let rs = read_table(
            chain,
            "DBFilesClient\\CreatureSoundData.dbc",
            &u32_columns::<30>(),
        )?;
        let rows = rs
            .records()
            .iter()
            .filter_map(|r| {
                let col = |i| u32_at(r, i).unwrap_or(0);
                let voice = Voice {
                    exertion: [col(1), col(2)],
                    injury: [col(3), col(4), col(5)],
                    death: col(6),
                    footstep_class: col(9),
                    custom_attack: [col(18), col(19), col(20), col(21)],
                    impact_type: col(24),
                };
                Some((u32_at(r, 0)?, voice))
            })
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
            rows,
        })
    }

    /// The display's row; `None` without one.
    pub fn voice(&self, display: u32) -> Option<&Voice> {
        self.rows.get(self.display_to_sound.get(&display)?)
    }

    /// The display's footstep class; `None` without a sound row.
    pub fn footstep_class(&self, display: u32) -> Option<u32> {
        self.voice(display).map(|v| v.footstep_class)
    }
}
