//! Cold breath: the puff of vapour a unit breathes out at the `$BTH` key of its idle clips, where
//! its area is cold.

use std::collections::{HashMap, HashSet};

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use mpq::Chain;

use super::ViewerUnit;
use super::body::UnitBody;
use super::look::CharacterTables;
use super::one_shot::{MOUTH, OneShot};
use crate::adt::AdtTile;
use crate::coords::bevy_to_wow;
use crate::dbc_table::{read_table, str_at, u32_at};
use crate::ground::{Ground, ground_under};
use crate::interior::CurrentArea;
use crate::m2::M2Model;
use crate::rig_events::AnimEvent;
use crate::source::m2_url;
use crate::stream::Streamer;

const BREATH_KEY: [u8; 4] = *b"$BTH";
/// The `SpellVisualEffectName` row the client breathes in the cold.
const COLD_BREATH_EFFECT: &str = "HARDCODED Breath Cold";
/// How long the client keeps a unit's climate, seconds.
const RECLASSIFY_SECS: f32 = 10.0;
/// `AreaTable.Flags`: the area is cold.
const COLD: u32 = 0x1;
/// `AreaTable.Flags`: the area's climate is its own, not its zone's.
const OWN_CLIMATE: u32 = 0x2;

#[derive(Resource)]
pub(crate) struct BreathTables {
    cold: HashSet<u32>,
    puff: Handle<M2Model>,
}

impl BreathTables {
    fn read(chain: &Chain, load: impl FnOnce(String) -> Handle<M2Model>) -> Result<Self, String> {
        let rs = read_table(chain, "DBFilesClient\\AreaTable.dbc", 25, &[11])?;
        let areas: HashMap<u32, (u32, u32)> = rs
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, (u32_at(r, 2)?, u32_at(r, 4)?))))
            .collect();
        let cold = areas
            .keys()
            .copied()
            .filter(|&id| is_cold(&areas, id))
            .collect();
        let rs = read_table(
            chain,
            "DBFilesClient\\SpellVisualEffectName.dbc",
            5,
            &[1, 2],
        )?;
        let puff = rs
            .records()
            .iter()
            .find(|r| str_at(&rs, r, 1).eq_ignore_ascii_case(COLD_BREATH_EFFECT))
            .map(|r| str_at(&rs, r, 2))
            .filter(|path| !path.is_empty())
            .ok_or_else(|| format!("no {COLD_BREATH_EFFECT:?} effect"))?;
        Ok(Self {
            cold,
            puff: load(m2_url(&puff)),
        })
    }
}

/// Whether an area is cold by its own flags when it sets [`OWN_CLIMATE`] or has no zone above
/// it, else by its zone's: one step up, never further.
fn is_cold(areas: &HashMap<u32, (u32, u32)>, id: u32) -> bool {
    let Some(&(zone, flags)) = areas.get(&id) else {
        return false;
    };
    let flags = match areas.get(&zone) {
        Some(&(_, zone_flags)) if flags & OWN_CLIMATE == 0 => zone_flags,
        _ => flags,
    };
    flags & COLD != 0
}

pub(crate) fn load_tables(
    mut commands: Commands<'_, '_>,
    install: Res<'_, crate::Install>,
    server: Res<'_, AssetServer>,
) {
    match BreathTables::read(&install.0, |url| server.load(url)) {
        Ok(tables) => {
            commands.insert_resource(tables);
        }
        Err(e) => warn!("no breath tables, so no breath in the cold: {e}"),
    }
}

#[derive(Component)]
pub(crate) struct BreathEnv {
    cold: bool,
    stale_at: f32,
}

type Breathers<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        Option<&'static BreathEnv>,
        Has<ViewerUnit>,
    ),
    With<UnitBody>,
>;

pub(crate) fn classify_breath(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    tables: Option<Res<'_, BreathTables>>,
    current: Res<'_, CurrentArea>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    units: Breathers<'_, '_>,
) {
    let Some(tables) = tables else {
        return;
    };
    let now = time.elapsed_secs();
    for (unit, at, env, viewer) in &units {
        if env.is_some_and(|e| now < e.stale_at) {
            continue;
        }
        let area = if viewer {
            current.0
        } else {
            match ground_under(&ground.0, &ground.1, at.translation()) {
                Ground::Tile(adt) => {
                    terrain::area_id_at(&adt.chunks, bevy_to_wow(at.translation()))
                }
                _ => None,
            }
        };
        let Some(area) = area else {
            continue;
        };
        commands.entity(unit).try_insert(BreathEnv {
            cold: tables.cold.contains(&area),
            stale_at: now + RECLASSIFY_SECS,
        });
    }
}

/// A puff at the mouth for each breath key a unit's clip crosses in the cold, never over one still
/// playing.
pub(crate) fn fire_breath(
    mut commands: Commands<'_, '_>,
    mut events: MessageReader<'_, '_, AnimEvent>,
    creatures: Option<Res<'_, CharacterTables>>,
    tables: Option<Res<'_, BreathTables>>,
    units: Query<'_, '_, (&UnitBody, Option<&BreathEnv>)>,
    shots: Query<'_, '_, &OneShot>,
) {
    let (Some(creatures), Some(tables)) = (creatures, tables) else {
        events.clear();
        return;
    };
    let mut puffing: EntityHashSet = shots
        .iter()
        .filter(|s| s.model == tables.puff)
        .map(|s| s.host)
        .collect();
    for ev in events.read() {
        if ev.ident != BREATH_KEY {
            continue;
        }
        let Ok((body, env)) = units.get(ev.entity) else {
            continue;
        };
        let cold = env.is_some_and(|e| e.cold);
        if cold && creatures.creatures.breathes(body.display) && puffing.insert(ev.entity) {
            commands.spawn(OneShot::new(tables.puff.clone(), ev.entity, MOUTH));
        }
    }
}

#[cfg(test)]
mod tests;
