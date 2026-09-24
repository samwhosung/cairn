use std::collections::HashMap;

use bevy::prelude::*;
use protocol::{Appearance, Jump, Record, State, flags};
use world::Install;
use world::coords::wow_to_bevy;
use world::unit::{
    BodySkin, CharacterLook, CharacterTables, UnitAlpha, UnitAppear, UnitMotion, UnitShade,
};

use super::remote::{RelayMove, RemoteMotion, swim_body_rotation};
use crate::player::character_body;
use crate::player::gait::wrap_pi;

const LEAVE_SECS: f32 = 2.0;
const LEAVE_MIN_ALPHA: f32 = 0.01;

#[derive(Component, Clone, Debug)]
pub struct OtherPlayer {
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub id: u32,
    pub name: String,
}

#[derive(Component)]
pub(super) struct Undressed(CharacterLook);

#[derive(Component)]
pub(super) struct Leaving {
    from_alpha: f32,
    started_secs: f32,
}

#[derive(Clone, Copy, Debug)]
struct Relayed {
    entity: Entity,
    wow_pos: [f32; 3],
    facing: f32,
    flags: u32,
    pitch: f32,
    jump: Option<Jump>,
    launched_server_ms: i64,
}

impl Relayed {
    fn of(entity: Entity, state: &State, server_ms: u32, own_pos: [f32; 3]) -> Self {
        let falling = state.flags & flags::FALLING != 0;
        Self {
            entity,
            wow_pos: state.pos.around(own_pos).yards(),
            facing: state.facing.radians(),
            flags: state.flags,
            pitch: if state.flags & flags::SWIMMING != 0 {
                wrap_pi(state.pitch.radians())
            } else {
                0.0
            },
            jump: falling.then_some(state.jump),
            launched_server_ms: i64::from(server_ms) - i64::from(state.fall_time),
        }
    }

    fn relay_move(&self, server_ms: u32) -> RelayMove {
        let fall_time = if self.flags & flags::FALLING != 0 {
            (i64::from(server_ms) - self.launched_server_ms).max(0) as u32
        } else {
            0
        };
        RelayMove {
            server_ms,
            wow_pos: self.wow_pos,
            orientation: self.facing,
            flags: self.flags,
            pitch: self.pitch,
            fall_time,
            jump: self.jump,
        }
    }
}

#[derive(Default)]
pub struct Others {
    by_slot: HashMap<u16, Relayed>,
    #[cfg(test)]
    pub faults: Faults,
    #[cfg(test)]
    dropped: bool,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Faults {
    pub drop_every_other: bool,
    pub no_dead_reckoning: bool,
}

pub struct BatchContext {
    pub server_ms: u32,
    pub own_pos: [f32; 3],
    pub arrived_real_ms: f64,
    pub now_real_ms: f64,
    pub game_secs: f32,
}

impl Others {
    pub fn take(&mut self, commands: &mut Commands<'_, '_>, record: Record<'_>, at: &BatchContext) {
        match record {
            Record::Appear {
                slot,
                id,
                name,
                appearance,
                state,
            } => {
                if let Some(old) = self.by_slot.remove(&slot) {
                    leave(commands, old.entity, at.game_secs);
                }
                info!("{name} comes into view");
                let entity = commands.spawn_empty().id();
                let relayed = Relayed::of(entity, &state, at.server_ms, at.own_pos);
                let mv = relayed.relay_move(at.server_ms);
                commands.entity(entity).insert((
                    OtherPlayer {
                        id,
                        name: name.to_owned(),
                    },
                    Undressed(look_of(&appearance)),
                    Transform::from_translation(wow_to_bevy(mv.wow_pos))
                        .with_rotation(swim_body_rotation(mv.orientation, mv.flags, mv.pitch)),
                    Visibility::default(),
                    UnitShade::default(),
                    UnitMotion::default(),
                    UnitAlpha::default(),
                    RemoteMotion::seeded(&mv, at.arrived_real_ms),
                ));
                self.by_slot.insert(slot, relayed);
            }
            Record::Vanish { slot } => {
                if let Some(gone) = self.by_slot.remove(&slot) {
                    leave(commands, gone.entity, at.game_secs);
                }
            }
            Record::Move { slot, pos, facing } => self.relay(commands, slot, at, |r| {
                r.wow_pos = pos.around(at.own_pos).yards();
                r.facing = facing.radians();
            }),
            Record::Turn { slot, facing } => self.relay(commands, slot, at, |r| {
                r.facing = facing.radians();
            }),
            Record::State { slot, state } => self.relay(commands, slot, at, |r| {
                *r = Relayed::of(r.entity, &state, at.server_ms, at.own_pos);
            }),
            Record::Correct { .. } => {}
        }
    }

    fn relay(
        &mut self,
        commands: &mut Commands<'_, '_>,
        slot: u16,
        at: &BatchContext,
        change: impl FnOnce(&mut Relayed),
    ) {
        let Some(r) = self.by_slot.get_mut(&slot) else {
            return;
        };
        change(r);
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut mv = r.relay_move(at.server_ms);
        let (arrived_ms, now_ms) = (at.arrived_real_ms, at.now_real_ms);
        #[cfg(test)]
        {
            self.dropped = self.faults.drop_every_other && !self.dropped;
            if self.dropped {
                return;
            }
            if self.faults.no_dead_reckoning {
                (mv.flags, mv.jump) = (0, None);
            }
        }
        commands
            .entity(r.entity)
            .queue(move |mut entity: EntityWorldMut<'_>| {
                if let Some(mut rm) = entity.get_mut::<RemoteMotion>() {
                    rm.relayed(mv, arrived_ms, now_ms);
                }
            });
    }

    pub fn leave_all(&mut self, commands: &mut Commands<'_, '_>, game_secs: f32) {
        for (_, gone) in self.by_slot.drain() {
            leave(commands, gone.entity, game_secs);
        }
    }
}

fn look_of(a: &Appearance) -> CharacterLook {
    CharacterLook {
        race: a.race,
        sex: a.sex,
        skin: a.skin,
        face: a.face,
        hair_style: a.hair_style,
        hair_color: a.hair_color,
        facial_hair: a.facial_hair,
        body: BodySkin::Composite,
        equipment: a.equipment,
    }
}

fn leave(commands: &mut Commands<'_, '_>, entity: Entity, game_secs: f32) {
    commands
        .entity(entity)
        .queue(move |mut e: EntityWorldMut<'_>| {
            if let Some(p) = e.get::<OtherPlayer>() {
                info!("{} leaves view", p.name);
            }
            let drawn = e.get::<UnitAlpha>().map_or(1.0, |a| a.alpha);
            let from_alpha = e.get::<UnitAppear>().map_or(drawn, |a| a.alpha(game_secs));
            e.remove::<(OtherPlayer, UnitAppear)>();
            if from_alpha < LEAVE_MIN_ALPHA {
                e.despawn();
            } else {
                e.insert(Leaving {
                    from_alpha,
                    started_secs: game_secs,
                });
            }
        });
}

pub(super) fn fade_leaving(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    mut leaving: Query<'_, '_, (Entity, &Leaving, &mut UnitAlpha)>,
) {
    for (entity, l, mut alpha) in &mut leaving {
        let t = (time.elapsed_secs() - l.started_secs) / LEAVE_SECS;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let s = ((1.0 - t) * l.from_alpha).clamp(0.0, 1.0);
        alpha.alpha = (3.0 - 2.0 * s) * s * s;
    }
}

pub(super) fn dress_remotes(
    mut commands: Commands<'_, '_>,
    tables: Option<Res<'_, CharacterTables>>,
    install: Res<'_, Install>,
    server: Res<'_, AssetServer>,
    mut images: ResMut<'_, Assets<Image>>,
    mut undressed: Query<'_, '_, (Entity, &Undressed, &mut Transform)>,
) {
    let Some(tables) = tables else {
        return;
    };
    for (entity, look, mut t) in &mut undressed {
        commands.entity(entity).remove::<Undressed>();
        if let Some(dressed) = character_body(&tables, &look.0, &install, &mut images, &server) {
            t.scale = Vec3::splat(dressed.scale);
            commands.entity(entity).insert(dressed.body);
        } else {
            warn!("no body for {:?}: another player walks unseen", look.0);
        }
    }
}

#[cfg(test)]
mod tests;
