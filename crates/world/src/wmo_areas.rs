use std::collections::HashMap;

use bevy::prelude::*;
use mpq::Chain;

use crate::dbc_table::{read_table, str_at, u32_at};

const TABLE: &str = "DBFilesClient\\WMOAreaTable.dbc";

/// One `WMOAreaTable` row, or a group's row with its building's default row under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WmoArea {
    pub id: u32,
    /// `SoundProviderPreferences` ids, `[dry, underwater]`.
    pub sound_provider: [u32; 2],
    pub ambience: u32,
    pub zone_music: u32,
    /// The `ZoneIntroMusicTable` fanfare.
    pub intro_sound: u32,
    pub area_table_id: u32,
    pub name: String,
}

/// The rows by `(WMOID, NameSetID, WMOGroupID)`, and each building's default row.
#[derive(Resource, Clone, Debug, Default)]
pub struct WmoAreas {
    groups: HashMap<(u32, u32, u32), WmoArea>,
    defaults: HashMap<(u32, u32), WmoArea>,
}

impl WmoAreas {
    pub fn load(chain: &Chain) -> Result<Self, String> {
        let rs = read_table(chain, TABLE, 20, &[11])?;
        let mut areas = Self::default();
        for r in rs.records() {
            let g = |i| u32_at(r, i).unwrap_or(0);
            let area = WmoArea {
                id: g(0),
                sound_provider: [g(4), g(5)],
                ambience: g(6),
                zone_music: g(7),
                intro_sound: g(8),
                area_table_id: g(10),
                name: str_at(&rs, r, 11),
            };
            if g(3) == u32::MAX {
                areas.defaults.insert((g(1), g(2)), area);
            } else {
                areas.groups.insert((g(1), g(2), g(3)), area);
            }
        }
        Ok(areas)
    }

    /// The group's row over its building's default row, each column the group leaves zero taken
    /// from the default; the building's rows in name set 0 when `name_set` has none.
    pub fn resolve(&self, wmo_id: u32, name_set: u32, group_id: u32) -> Option<WmoArea> {
        let (group, default) = [name_set, 0]
            .iter()
            .map(|&ns| {
                (
                    self.groups.get(&(wmo_id, ns, group_id)),
                    self.defaults.get(&(wmo_id, ns)),
                )
            })
            .find(|(g, d)| g.is_some() || d.is_some())?;
        let base = default.cloned().unwrap_or_default();
        let Some(g) = group else {
            return Some(base);
        };
        let nz = |v: u32, b: u32| if v == 0 { b } else { v };
        Some(WmoArea {
            id: g.id,
            sound_provider: [
                nz(g.sound_provider[0], base.sound_provider[0]),
                nz(g.sound_provider[1], base.sound_provider[1]),
            ],
            ambience: nz(g.ambience, base.ambience),
            zone_music: nz(g.zone_music, base.zone_music),
            intro_sound: nz(g.intro_sound, base.intro_sound),
            area_table_id: nz(g.area_table_id, base.area_table_id),
            name: if g.name.is_empty() {
                base.name
            } else {
                g.name.clone()
            },
        })
    }

    pub fn len(&self) -> usize {
        self.groups.len() + self.defaults.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub(crate) fn load(mut commands: Commands<'_, '_>, install: Res<'_, crate::Install>) {
    match WmoAreas::load(&install.0) {
        Ok(areas) => {
            commands.insert_resource(areas);
        }
        Err(e) => warn!("no WMOAreaTable, so no building has an area of its own: {e}"),
    }
}
