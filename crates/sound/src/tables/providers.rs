use std::collections::HashMap;

use dbc::FieldType;
use mpq::Chain;

use super::{Error, f32_at, read_table, str_at, u32_at};

/// One `SoundProviderPreferences` row: an EAX 2 listener preset, as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundProvider {
    pub id: u32,
    pub name: String,
    pub flags: u32,
    /// Seconds.
    pub decay_time: f32,
    /// Millibels.
    pub room: i32,
    pub room_hf: i32,
    pub decay_hf_ratio: f32,
    pub reflections: i32,
    pub reverb: i32,
    pub env_diffusion: f32,
    pub env_size: f32,
}

/// The reverb presets by id.
#[derive(Clone, Debug, Default, bevy::prelude::Resource)]
pub struct SoundProviders {
    providers: HashMap<u32, SoundProvider>,
}

impl SoundProviders {
    pub fn load(chain: &Chain) -> Result<Self, Error> {
        use FieldType::{Float32 as F, String as S, UInt32 as U};
        let mut columns = vec![
            ("ID", U),
            ("Description", S),
            ("Flags", U),
            ("EnvironmentSelection", U),
            ("DecayTime", F),
            ("EnvironmentSize", F),
            ("EnvironmentDiffusion", F),
            ("Room", U),
            ("RoomHF", U),
            ("DecayHFRatio", F),
            ("Reflections", U),
            ("ReflectionsDelay", F),
            ("Reverb", U),
            ("ReverbDelay", F),
            ("RoomRolloff", F),
            ("AirAbsorption", F),
        ];
        columns.extend([("", U); 8]);
        let rs = read_table(
            chain,
            "DBFilesClient\\SoundProviderPreferences.dbc",
            &columns,
        )?;
        let mut providers = HashMap::new();
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else { continue };
            let i32_at = |i| u32_at(r, i).unwrap_or(0) as i32;
            providers.insert(
                id,
                SoundProvider {
                    id,
                    name: str_at(&rs, r, 1),
                    flags: u32_at(r, 2).unwrap_or(0),
                    decay_time: f32_at(r, 4).unwrap_or(0.0),
                    room: i32_at(7),
                    room_hf: i32_at(8),
                    decay_hf_ratio: f32_at(r, 9).unwrap_or(1.0),
                    reflections: i32_at(10),
                    reverb: i32_at(12),
                    env_diffusion: f32_at(r, 6).unwrap_or(1.0),
                    env_size: f32_at(r, 5).unwrap_or(1.0),
                },
            );
        }
        Ok(Self { providers })
    }

    pub fn get(&self, id: u32) -> Option<&SoundProvider> {
        self.providers.get(&id)
    }

    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}
