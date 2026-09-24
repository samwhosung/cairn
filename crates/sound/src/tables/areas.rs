use std::collections::HashMap;

use dbc::FieldType;
use mpq::Chain;

use super::{Error, read_table, str_at, u32_at, u32_columns};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Area {
    parent: u32,
    ambience: u32,
    zone_music: u32,
    intro_sound: u32,
    sound_provider: [u32; 2],
    name: String,
}

/// One `ZoneMusic` row; each pair is `[day, night]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneMusic {
    pub id: u32,
    pub set_name: String,
    /// The silence between tracks, ms, uniform in `[min, max]`.
    pub silence_min: [u32; 2],
    pub silence_max: [u32; 2],
    /// The kits the tracks are picked from.
    pub sounds: [u32; 2],
}

/// One `ZoneIntroMusicTable` row: the fanfare on entering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneIntro {
    pub id: u32,
    pub sound_id: u32,
    pub priority: u32,
    /// The least time between two plays of this fanfare.
    pub min_delay_minutes: u32,
}

/// An area's audio after the walk up its parents.
#[derive(Clone, Copy, Debug)]
pub struct AreaAudio<'a> {
    pub name: &'a str,
    pub music: Option<&'a ZoneMusic>,
    pub intro: Option<&'a ZoneIntro>,
    /// The `SoundAmbience` kits, `[day, night]`.
    pub ambience: Option<[u32; 2]>,
    /// `SoundProviderPreferences` ids, `[dry, underwater]`; `0` where neither it nor a parent
    /// names one.
    pub sound_provider: [u32; 2],
}

/// `AreaTable` joined to `ZoneMusic`, `ZoneIntroMusicTable` and `SoundAmbience`.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct AreaSounds {
    areas: HashMap<u32, Area>,
    music: HashMap<u32, ZoneMusic>,
    intros: HashMap<u32, ZoneIntro>,
    ambience: HashMap<u32, [u32; 2]>,
}

const PARENT_WALK: usize = 8;

impl AreaSounds {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        let mut cat = Self::default();
        let mut columns = u32_columns::<25>().to_vec();
        columns[11] = ("AreaName", FieldType::String);
        let rs = read_table(chain, "DBFilesClient\\AreaTable.dbc", &columns)?;
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let g = |i| u32_at(r, i).unwrap_or(0);
            cat.areas.insert(
                id,
                Area {
                    parent: g(2),
                    ambience: g(7),
                    zone_music: g(8),
                    intro_sound: g(9),
                    sound_provider: [g(5), g(6)],
                    name: str_at(&rs, r, 11),
                },
            );
        }
        let mut columns = u32_columns::<8>().to_vec();
        columns[1] = ("SetName", FieldType::String);
        let rs = read_table(chain, "DBFilesClient\\ZoneMusic.dbc", &columns)?;
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let g = |i| u32_at(r, i).unwrap_or(0);
            cat.music.insert(
                id,
                ZoneMusic {
                    id,
                    set_name: str_at(&rs, r, 1),
                    silence_min: [g(2), g(3)],
                    silence_max: [g(4), g(5)],
                    sounds: [g(6), g(7)],
                },
            );
        }
        let mut columns = u32_columns::<5>().to_vec();
        columns[1] = ("Name", FieldType::String);
        let rs = read_table(chain, "DBFilesClient\\ZoneIntroMusicTable.dbc", &columns)?;
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let g = |i| u32_at(r, i).unwrap_or(0);
            cat.intros.insert(
                id,
                ZoneIntro {
                    id,
                    sound_id: g(2),
                    priority: g(3),
                    min_delay_minutes: g(4),
                },
            );
        }
        let rs = read_table(
            chain,
            "DBFilesClient\\SoundAmbience.dbc",
            &u32_columns::<3>(),
        )?;
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let g = |i| u32_at(r, i).unwrap_or(0);
            cat.ambience.insert(id, [g(1), g(2)]);
        }
        Ok(cat)
    }

    pub fn zone_music(&self, id: u32) -> Option<&ZoneMusic> {
        self.music.get(&id)
    }

    pub fn intro(&self, id: u32) -> Option<&ZoneIntro> {
        self.intros.get(&id)
    }

    /// A `SoundAmbience` row's `[day, night]` kits.
    pub fn ambience(&self, id: u32) -> Option<[u32; 2]> {
        self.ambience.get(&id).copied()
    }

    /// Each audio column from the nearest area up the parents that sets it.
    pub fn resolve(&self, area_id: u32) -> Option<AreaAudio<'_>> {
        let first = self.areas.get(&area_id)?;
        let mut out = AreaAudio {
            name: &first.name,
            music: None,
            intro: None,
            ambience: None,
            sound_provider: [0; 2],
        };
        let mut cur = Some(first);
        for _ in 0..PARENT_WALK {
            let Some(a) = cur else { break };
            if out.music.is_none() && a.zone_music != 0 {
                out.music = self.music.get(&a.zone_music);
            }
            if out.intro.is_none() && a.intro_sound != 0 {
                out.intro = self.intros.get(&a.intro_sound);
            }
            if out.ambience.is_none() && a.ambience != 0 {
                out.ambience = self.ambience.get(&a.ambience).copied();
            }
            for (slot, own) in out.sound_provider.iter_mut().zip(a.sound_provider) {
                if *slot == 0 {
                    *slot = own;
                }
            }
            if a.parent == 0 {
                break;
            }
            cur = self.areas.get(&a.parent);
        }
        Some(out)
    }

    pub fn area_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.areas.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.areas.len()
    }

    pub fn is_empty(&self) -> bool {
        self.areas.is_empty()
    }
}
