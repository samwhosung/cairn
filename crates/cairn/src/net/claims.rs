//! The player's own movement, claimed to the server as the client's cadence says, the teleports
//! it asks for, and the corrections the server answers a refused claim or teleport with.

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
    pub corrections: u32,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub told: Option<Why>,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub sent: u32,
    #[cfg_attr(not(test), allow(dead_code, reason = "the scenarios read it"))]
    pub teleports: u32,
}

impl Claims {
    pub fn new(spawn: &Movement) -> Self {
        Self {
            cadence: Cadence::new(spawn),
            ack: 0,
            arc_began: None,
            corrections: 0,
            told: None,
            sent: 0,
            teleports: 0,
        }
    }

    pub fn correct(&mut self, player: &mut Player, seq: u32, why: Why, movement: &Movement) {
        let put = wow_to_bevy(movement.pos);
        let past_the_streamed_collision = put.distance(player.pos) > world::FARCLIP;
        player.pos = put;
        player.face_yaw = movement.facing;
        player.model_yaw = movement.facing;
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
        player.airborne_since = None;
        player.settling |= past_the_streamed_collision;
        self.ack = seq;
        self.cadence.report_now();
        self.corrections += 1;
        self.told = Some(why);
    }
}

pub fn movement_of(
    player: &Player,
    flying: bool,
    now: Duration,
    arc_began: Option<f32>,
) -> Movement {
    let secs = now.as_secs_f32();
    let flags = if flying { 0 } else { player.move_flags };
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
        link.send(&ClientMessage::Teleport(claim));
        claims.teleports += 1;
        return;
    }
    for _ in 0..claims.cadence.claims(&movement) {
        link.send(&ClientMessage::Claim(claim));
        claims.sent += 1;
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
