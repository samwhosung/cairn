//! The player's own movement, claimed to the server as the client's cadence says, and the
//! corrections the server answers a refused claim with.

use std::f32::consts::TAU;
use std::time::Duration;

use bevy::math::ops;
use bevy::prelude::*;
use protocol::{Cadence, Claim, ClientMessage, Jump, Movement, flags};
use world::coords::{bevy_to_wow, wow_to_bevy};

use super::Net;
use crate::player::{Mode, Player};

/// What a joined window's claims carry from frame to frame.
pub struct Claims {
    cadence: Cadence,
    ack: u32,
    /// When the arc under way began, seconds: a landing claims how long it lasted.
    arc_began: Option<f32>,
}

impl Claims {
    pub fn new(spawn: &Movement) -> Self {
        Self {
            cadence: Cadence::new(spawn),
            ack: 0,
            arc_began: None,
        }
    }

    /// Puts the player where a refused claim left it on the server, and acknowledges it.
    pub fn correct(&mut self, player: &mut Player, seq: u32, movement: &Movement) {
        player.pos = wow_to_bevy(movement.pos);
        player.face_yaw = movement.facing;
        player.model_yaw = movement.facing;
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
        player.airborne_since = None;
        self.ack = seq;
        self.cadence.report_now();
    }
}

/// The player's movement this frame as a claim carries it; a body the window flies away from
/// stands still.
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
    mut net: ResMut<'_, Net>,
) {
    let Net { claims, link, .. } = &mut *net;
    let Some(claims) = claims else {
        return;
    };
    let flying = *mode == Mode::Fly;
    let movement = movement_of(&player, flying, time.elapsed(), claims.arc_began);
    claims.arc_began = player.airborne_since;
    let claim = ClientMessage::Claim(Claim {
        ack: claims.ack,
        movement,
    });
    for _ in 0..claims.cadence.claims(&movement) {
        link.send(&claim);
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
