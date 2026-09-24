//! Other players in view: each appears as the server introduces it and is drawn in its look as
//! the player's own body is, moves as its relayed moves say, and fades out when it leaves.

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

/// How long a player who left takes to fade out, seconds.
const LEAVE_SECS: f32 = 2.0;
/// A body leaving fainter than this goes at once.
const LEAVE_MIN_ALPHA: f32 = 0.01;

/// Another player in view, as the server named it.
#[derive(Component, Clone, Debug)]
#[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
pub struct Remote {
    pub id: u32,
    pub name: String,
}

/// The look another player walks in, until its body is dressed.
#[derive(Component)]
pub(super) struct Undressed(CharacterLook);

/// A body fading out after its player left, from the alpha it was drawn at, since `started`.
#[derive(Component)]
pub(super) struct Leaving {
    from: f32,
    started: f32,
}

/// What the server last relayed of one player: a move or a turn repeats only its position and
/// facing, and the rest stands as it was.
#[derive(Clone, Copy, Debug)]
struct Relayed {
    entity: Entity,
    position: [f32; 3],
    facing: f32,
    flags: u32,
    pitch: f32,
    jump: Option<Jump>,
    /// When the arc under way was launched, on the server's clock, ms.
    launched_ms: i64,
}

impl Relayed {
    fn of(entity: Entity, state: &State, wire_ms: u32, me: [f32; 3]) -> Self {
        let falling = state.flags & flags::FALLING != 0;
        Self {
            entity,
            position: state.pos.around(me).yards(),
            facing: state.facing.radians(),
            flags: state.flags,
            pitch: if state.flags & flags::SWIMMING != 0 {
                wrap_pi(state.pitch.radians())
            } else {
                0.0
            },
            jump: falling.then_some(state.jump),
            launched_ms: i64::from(wire_ms) - i64::from(state.fall_time),
        }
    }

    fn relay_move(&self, wire_ms: u32) -> RelayMove {
        let fall_time = if self.flags & flags::FALLING != 0 {
            (i64::from(wire_ms) - self.launched_ms).max(0) as u32
        } else {
            0
        };
        RelayMove {
            wire_ms,
            position: self.position,
            orientation: self.facing,
            flags: self.flags,
            pitch: self.pitch,
            fall_time,
            jump: self.jump,
        }
    }
}

/// The players in view, by the slot the server gave each.
#[derive(Default)]
pub struct Others {
    by_slot: HashMap<u16, Relayed>,
}

/// What a batch's records need beyond themselves: the batch's stamp, where this window's player
/// stands (relayed positions are unwrapped around it), and the clocks.
pub struct Stamp {
    pub wire_ms: u32,
    pub me: [f32; 3],
    /// The replay clock, ms.
    pub now_ms: f64,
    /// The frame clock, s.
    pub now_secs: f32,
}

impl Others {
    /// Takes in one record of a batch; a correction is not this one's.
    pub fn take(&mut self, commands: &mut Commands<'_, '_>, record: Record<'_>, at: &Stamp) {
        match record {
            Record::Appear {
                slot,
                id,
                name,
                appearance,
                state,
            } => {
                if let Some(old) = self.by_slot.remove(&slot) {
                    leave(commands, old.entity, at.now_secs);
                }
                let entity = commands.spawn_empty().id();
                let relayed = Relayed::of(entity, &state, at.wire_ms, at.me);
                let mv = relayed.relay_move(at.wire_ms);
                commands.entity(entity).insert((
                    Remote {
                        id,
                        name: name.to_owned(),
                    },
                    Undressed(look_of(&appearance)),
                    Transform::from_translation(wow_to_bevy(mv.position))
                        .with_rotation(swim_body_rotation(mv.orientation, mv.flags, mv.pitch)),
                    Visibility::default(),
                    UnitShade::default(),
                    UnitMotion::default(),
                    UnitAlpha::default(),
                    RemoteMotion::seeded(&mv, at.now_ms),
                ));
                self.by_slot.insert(slot, relayed);
            }
            Record::Vanish { slot } => {
                if let Some(gone) = self.by_slot.remove(&slot) {
                    leave(commands, gone.entity, at.now_secs);
                }
            }
            Record::Move { slot, pos, facing } => self.relay(commands, slot, at, |r| {
                r.position = pos.around(at.me).yards();
                r.facing = facing.radians();
            }),
            Record::Turn { slot, facing } => self.relay(commands, slot, at, |r| {
                r.facing = facing.radians();
            }),
            Record::State { slot, state } => self.relay(commands, slot, at, |r| {
                *r = Relayed::of(r.entity, &state, at.wire_ms, at.me);
            }),
            Record::Correct { .. } => {}
        }
    }

    fn relay(
        &mut self,
        commands: &mut Commands<'_, '_>,
        slot: u16,
        at: &Stamp,
        change: impl FnOnce(&mut Relayed),
    ) {
        let Some(r) = self.by_slot.get_mut(&slot) else {
            return;
        };
        change(r);
        let (mv, now_ms) = (r.relay_move(at.wire_ms), at.now_ms);
        commands
            .entity(r.entity)
            .queue(move |mut entity: EntityWorldMut<'_>| {
                if let Some(mut rm) = entity.get_mut::<RemoteMotion>() {
                    rm.relayed(mv, now_ms);
                }
            });
    }

    /// Every player in view leaves.
    pub fn leave_all(&mut self, commands: &mut Commands<'_, '_>, now_secs: f32) {
        for (_, gone) in self.by_slot.drain() {
            leave(commands, gone.entity, now_secs);
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

/// The body stops being a player at once and fades out from the alpha it was drawn at.
fn leave(commands: &mut Commands<'_, '_>, entity: Entity, now: f32) {
    commands
        .entity(entity)
        .queue(move |mut e: EntityWorldMut<'_>| {
            let drawn = e.get::<UnitAlpha>().map_or(1.0, |a| a.alpha);
            let from = e.get::<UnitAppear>().map_or(drawn, |a| a.alpha(now));
            e.remove::<(Remote, UnitAppear)>();
            if from < LEAVE_MIN_ALPHA {
                e.despawn();
            } else {
                e.insert(Leaving { from, started: now });
            }
        });
}

pub(super) fn fade_leaving(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    mut leaving: Query<'_, '_, (Entity, &Leaving, &mut UnitAlpha)>,
) {
    for (entity, l, mut alpha) in &mut leaving {
        let t = (time.elapsed_secs() - l.started) / LEAVE_SECS;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let s = ((1.0 - t) * l.from).clamp(0.0, 1.0);
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
        if let Some((body, scale)) =
            character_body(&tables, &look.0, &install, &mut images, &server)
        {
            t.scale = Vec3::splat(scale);
            commands.entity(entity).insert(body);
        } else {
            warn!("no body for {:?}: another player walks unseen", look.0);
        }
    }
}

#[cfg(test)]
mod tests;
