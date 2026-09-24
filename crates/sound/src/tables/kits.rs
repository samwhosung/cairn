use std::collections::HashMap;

use dbc::FieldType;
use mpq::Chain;

use super::{Error, f32_at, read_table, str_at, u32_at};

const SOUND_ENTRIES: &str = "DBFilesClient\\SoundEntries.dbc";

/// `SoundEntries.Flags`, copied raw into the kit's playback flags.
pub mod kit_flags {
    /// No second copy while one is audible.
    pub const NO_DUPLICATES: u32 = 0x20;
    pub const LOOPING: u32 = 0x200;
    pub const VARY_PITCH: u32 = 0x400;
    /// Set by no 1.12 row.
    pub const VARY_VOLUME: u32 = 0x800;
}

/// One `SoundEntries` row: up to ten weighted variation files and how they play.
#[derive(Clone, Debug, PartialEq)]
pub struct Kit {
    pub id: u32,
    pub sound_type: u32,
    /// The `PlaySoundByName` key.
    pub name: String,
    /// `(archive path, weight)` for every non-empty file slot, in slot order.
    pub files: Vec<(String, u32)>,
    pub volume: f32,
    pub flags: u32,
    /// Full volume inside this, yd; the rolloff's knee.
    pub min_distance: f32,
    /// Inaudible at or past this, yd; `0` never culls.
    pub distance_cutoff: f32,
    /// `SoundSamplePreferences` id. `0` names no row, and such a kit never takes reverb.
    pub eax_def: u32,
}

/// Every kit, by id and by case-insensitive name.
#[derive(Clone, Debug, Default)]
pub struct KitCatalog {
    kits: HashMap<u32, Kit>,
    by_name: HashMap<String, u32>,
}

impl KitCatalog {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let mut columns = vec![
            ("ID", FieldType::UInt32),
            ("SoundType", FieldType::UInt32),
            ("Name", FieldType::String),
        ];
        columns.extend([("File", FieldType::String); 10]);
        columns.extend([("Freq", FieldType::UInt32); 10]);
        columns.extend([
            ("DirectoryBase", FieldType::String),
            ("Volume", FieldType::Float32),
            ("Flags", FieldType::UInt32),
            ("MinDistance", FieldType::Float32),
            ("DistanceCutoff", FieldType::Float32),
            ("EAXDef", FieldType::UInt32),
        ]);
        let rs = read_table(chain, SOUND_ENTRIES, &columns)?;
        let mut cat = Self::default();
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let name = str_at(&rs, r, 2);
            let dir = str_at(&rs, r, 23);
            let files = (0..10)
                .filter_map(|i| {
                    let file = str_at(&rs, r, 3 + i);
                    (!file.is_empty())
                        .then(|| (join_variation(&dir, &file), u32_at(r, 13 + i).unwrap_or(0)))
                })
                .collect();
            if !name.is_empty() {
                cat.by_name.insert(name.to_ascii_lowercase(), id);
            }
            cat.kits.insert(
                id,
                Kit {
                    id,
                    sound_type: u32_at(r, 1).unwrap_or(0),
                    name,
                    files,
                    volume: f32_at(r, 24).unwrap_or(1.0),
                    flags: u32_at(r, 25).unwrap_or(0),
                    min_distance: f32_at(r, 26).unwrap_or(0.0),
                    distance_cutoff: f32_at(r, 27).unwrap_or(0.0),
                    eax_def: u32_at(r, 28).unwrap_or(0),
                },
            );
        }
        Ok(cat)
    }

    pub fn get(&self, id: u32) -> Option<&Kit> {
        self.kits.get(&id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Kit> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .and_then(|id| self.kits.get(id))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Kit> {
        self.kits.values()
    }

    pub fn len(&self) -> usize {
        self.kits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kits.is_empty()
    }
}

/// The client's join: no separator after an empty directory or one already ending in `\`, one
/// otherwise, and nothing else normalised — a leading `\` stays and misses in every archive.
fn join_variation(dir: &str, file: &str) -> String {
    if dir.is_empty() || dir.ends_with('\\') {
        format!("{dir}{file}")
    } else {
        format!("{dir}\\{file}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_join_adds_one_separator_and_normalises_nothing() {
        assert_eq!(
            join_variation("Sound\\Spells", "Dispel_Low_Base.wav"),
            "Sound\\Spells\\Dispel_Low_Base.wav"
        );
        assert_eq!(
            join_variation("Sound\\interface\\", "igNewTaxiNodeDiscovered.wav"),
            "Sound\\interface\\igNewTaxiNodeDiscovered.wav"
        );
        assert_eq!(
            join_variation("", "Sound\\Creature\\Wyvern\\WyvernWingFlap1.wav"),
            "Sound\\Creature\\Wyvern\\WyvernWingFlap1.wav"
        );
        assert_eq!(
            join_variation("\\Sound\\Creature\\Ashbringer\\", "ASH_SPEAK_01.wav"),
            "\\Sound\\Creature\\Ashbringer\\ASH_SPEAK_01.wav"
        );
    }
}
