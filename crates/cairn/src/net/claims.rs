//! The player's own movement, claimed to the server as the client's cadence says, the teleports
//! it asks for, and the server's answers, a correction or a grant, with where the server holds the
//! player until they come.

use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::time::Duration;

use bevy::math::ops;
use bevy::prelude::*;
use protocol::{Cadence, Claim, ClientMessage, Jump, Movement, Why, flags};
use world::coords::{bevy_to_wow, wow_to_bevy};

use super::Net;
use crate::player::{Mode, Player, Teleported};

pub struct Claims {
    cadence: Cadence,
    ack: u32,
    /// When the arc under way began, seconds: a landing claims how long it lasted.
    arc_began: Option<f32>,
    claimed: [f32; 3],
    awaiting: Option<Awaiting>,
    pub corrections: u32,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub why_put_back: Option<Why>,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub sent: u32,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub teleports: u32,
    pub placements: u32,
}

struct Awaiting {
    held: [f32; 3],
    clocks: VecDeque<u32>,
}

impl Claims {
    pub fn new(spawn: &Movement) -> Self {
        Self {
            cadence: Cadence::new(spawn),
            ack: 0,
            arc_began: None,
            claimed: spawn.pos,
            awaiting: None,
            corrections: 0,
            why_put_back: None,
            sent: 0,
            teleports: 0,
            placements: 0,
        }
    }

    pub fn correct(&mut self, player: &mut Player, seq: u32, why: Why, movement: &Movement) {
        self.put(player, seq, movement);
        self.corrections += 1;
        self.why_put_back = Some(why);
    }

    pub fn place(&mut self, player: &mut Player, seq: u32, rooted: bool, movement: &Movement) {
        self.put(player, seq, movement);
        player.rooted = rooted;
        self.placements += 1;
    }

    fn put(&mut self, player: &mut Player, seq: u32, movement: &Movement) {
        let put = wow_to_bevy(movement.pos);
        let past_the_streamed_collision = put.distance(player.pos) > world::FARCLIP;
        player.put(put, movement.facing);
        player.settling |= past_the_streamed_collision;
        self.ack = seq;
        self.cadence.report_now();
        self.awaiting = None;
        self.claimed = movement.pos;
    }

    pub fn granted(&mut self, movement: &Movement) {
        if let Some(a) = &mut self.awaiting {
            while a.clocks.front().is_some_and(|&t| t <= movement.time) {
                a.clocks.pop_front();
            }
            a.held = movement.pos;
        }
        if self.awaiting.as_ref().is_some_and(|a| a.clocks.is_empty()) {
            self.awaiting = None;
        }
    }

    pub fn read_around(&self, stands: [f32; 3]) -> [f32; 3] {
        self.awaiting.as_ref().map_or(stands, |a| a.held)
    }

    pub(super) fn teleported(&mut self, movement: &Movement) {
        let held = self.claimed;
        let awaiting = self.awaiting.get_or_insert_with(|| Awaiting {
            held,
            clocks: VecDeque::new(),
        });
        awaiting.clocks.push_back(movement.time);
        self.claimed = movement.pos;
    }
}

pub fn movement_of(
    player: &Player,
    flying: bool,
    now: Duration,
    arc_began: Option<f32>,
) -> Movement {
    let secs = now.as_secs_f32();
    let rooted = if player.rooted { flags::ROOT } else { 0 };
    let flags = rooted | if flying { 0 } else { player.move_flags };
    let facing = player.face_yaw.rem_euclid(TAU);
    let airborne = |since: f32| ((secs - since).max(0.0) * 1000.0).round() as u32;
    let jump = if flags & flags::FALLING != 0 {
        let v = bevy_to_wow(player.horiz_vel);
        let xy_speed = v[0].hypot(v[1]);
        let (cos, sin) = if xy_speed > 1.0e-4 {
            (v[0] / xy_speed, v[1] / xy_speed)
        } else {
            (ops::cos(facing), ops::sin(facing))
        };
        Jump {
            z_speed: -player.jump_zspeed,
            cos,
            sin,
            xy_speed,
        }
    } else {
        Jump::default()
    };
    Movement {
        time: now.as_millis() as u32,
        flags,
        pos: bevy_to_wow(player.pos),
        facing,
        pitch: if flags & flags::SWIMMING != 0 {
            player.swim_pitch
        } else {
            0.0
        },
        fall_time: player.airborne_since.or(arc_began).map_or(0, airborne),
        jump,
    }
}

pub(super) fn claim(
    time: Res<'_, Time>,
    mode: Res<'_, Mode>,
    player: Res<'_, Player>,
    mut teleported: MessageReader<'_, '_, Teleported>,
    mut net: ResMut<'_, Net>,
) {
    let teleported = teleported.read().count() > 0;
    let Net { claims, link, .. } = &mut *net;
    let Some(claims) = claims else {
        return;
    };
    let flying = *mode == Mode::Fly;
    let movement = movement_of(&player, flying, time.elapsed(), claims.arc_began);
    claims.arc_began = player.airborne_since;
    let claim = Claim {
        ack: claims.ack,
        movement,
    };
    if teleported {
        claims.cadence = Cadence::new(&movement);
        claims.teleported(&movement);
        link.send(&ClientMessage::Teleport(claim));
        claims.teleports += 1;
        return;
    }
    for _ in 0..claims.cadence.claims(&movement) {
        link.send(&ClientMessage::Claim(claim));
        claims.sent += 1;
        claims.claimed = movement.pos;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running_jump() -> Player {
        Player {
            pos: wow_to_bevy([10.0, 20.0, 30.0]),
            face_yaw: -0.5,
            move_flags: flags::FORWARD | flags::FALLING,
            horiz_vel: wow_to_bevy([7.0, 0.0, 0.0]),
            jump_zspeed: 7.955_547,
            airborne_since: Some(1.0),
            ..Player::default()
        }
    }

    #[test]
    fn a_leap_claims_its_launch_down_positive_and_its_momentum_in_world_axes() {
        let m = movement_of(&running_jump(), false, Duration::from_millis(1250), None);
        assert_eq!((m.time, m.fall_time), (1250, 250));
        assert!((m.facing - (TAU - 0.5)).abs() < 1e-5, "{}", m.facing);
        assert!((m.pos[0] - 10.0).abs() < 1e-4 && (m.pos[2] - 30.0).abs() < 1e-4);
        let j = m.jump;
        assert!((j.z_speed + 7.955_547).abs() < 1e-5);
        assert!(
            (j.cos - 1.0).abs() < 1e-5 && j.sin.abs() < 1e-5 && (j.xy_speed - 7.0).abs() < 1e-4
        );
    }

    #[test]
    fn only_a_body_put_back_past_the_far_clip_waits_for_its_collision() {
        let mut claims = Claims::new(&Movement::default());
        let put = |x: f32| Movement {
            pos: [x, 0.0, 0.0],
            ..Movement::default()
        };
        let (mut near, mut far) = (Player::default(), Player::default());
        claims.correct(&mut near, 1, Why::Speed, &put(3.0));
        claims.correct(&mut far, 2, Why::Teleport, &put(500.0));
        assert_eq!((near.settling, far.settling), (false, true));
    }

    #[test]
    fn a_placed_body_stands_where_it_is_put_and_claims_its_root_until_freed() {
        let mut claims = Claims::new(&Movement::default());
        let mut player = running_jump();
        let at = Movement {
            pos: [40.0, 0.0, 5.0],
            facing: 1.0,
            ..Movement::default()
        };
        claims.place(&mut player, 3, true, &at);
        assert_eq!(
            (claims.ack, claims.placements, claims.corrections),
            (3, 1, 0)
        );
        let m = movement_of(&player, false, Duration::from_millis(2000), None);
        assert!((m.pos[0] - 40.0).abs() < 1e-4 && m.flags & flags::ROOT != 0);
        claims.place(&mut player, 4, false, &at);
        let m = movement_of(&player, false, Duration::from_millis(2100), None);
        assert_eq!(m.flags & flags::ROOT, 0);
    }

    #[test]
    fn a_landing_claims_the_arc_it_ends_and_a_flown_body_stands() {
        let landed = Player {
            move_flags: flags::FORWARD,
            airborne_since: None,
            ..running_jump()
        };
        let m = movement_of(&landed, false, Duration::from_millis(1800), Some(1.0));
        assert_eq!((m.fall_time, m.jump), (800, Jump::default()));
        let m = movement_of(&running_jump(), true, Duration::from_millis(1800), None);
        assert_eq!(m.flags, 0);
    }
}
