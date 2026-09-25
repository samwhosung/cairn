use std::collections::HashMap;

use mpq::Chain;

use super::{Error, impact_slot, read_table, u32_at, u32_columns};

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
    /// The attacker's grunt.
    pub exertion: Exertion,
    /// The cry of the one hit.
    pub injury: Injury,
    pub death: u32,
    /// A class of `FootstepTerrainLookup`, not a kit.
    pub footstep_class: u32,
    /// What a clip's `$AH0`..`$AH3` key sounds in place of a weapon's impact.
    pub custom_attack: [u32; 4],
    /// The [`impact_slot`] a weapon strikes in the body.
    pub struck_as: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Exertion {
    pub normal: u32,
    pub critical: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Injury {
    pub normal: u32,
    pub critical: u32,
    pub crushing: u32,
}

/// The client's slot for a `CreatureSoundData` impact type, flesh past the four it knows.
fn struck_as(impact_type: u32) -> usize {
    match impact_type {
        1 => impact_slot::STONE,
        2 => impact_slot::WOOD,
        3 => impact_slot::ETHEREAL,
        _ => impact_slot::FLESH,
    }
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
                    exertion: Exertion {
                        normal: col(1),
                        critical: col(2),
                    },
                    injury: Injury {
                        normal: col(3),
                        critical: col(4),
                        crushing: col(5),
                    },
                    death: col(6),
                    footstep_class: col(9),
                    custom_attack: [col(18), col(19), col(20), col(21)],
                    struck_as: struck_as(col(24)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_strikes_as_flesh_stone_wood_or_ethereal_and_past_those_as_flesh() {
        assert_eq!(
            [0, 1, 2, 3, 4, 99].map(struck_as),
            [
                impact_slot::FLESH,
                impact_slot::STONE,
                impact_slot::WOOD,
                impact_slot::ETHEREAL,
                impact_slot::FLESH,
                impact_slot::FLESH
            ]
        );
    }
}
