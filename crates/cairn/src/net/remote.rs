//! Another player's motion between the moves the server relays: flag-driven on the ground, a
//! jump played out as its launch's ballistic arc, a swimmer along its pitch, each step resolved
//! against the world's collision; a relayed move waits for the time its chain gives it, and the
//! frames before it blend the drawn pose onto it.

use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};

use bevy::math::ops;
use bevy::prelude::*;
use bevy::time::Real;
use protocol::{Jump, flags};
use world::collision::WorldCollision;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{BodyTwist, UnitMotion};

use super::relay::RelayChain;
use crate::player::PlayerCapsule;
use crate::player::gait::{ease_strafe_yaw, strafe_body_offset, wrap_pi};
use crate::player::mover::{Support, airborne_step, fall_step, grounded_step};
use crate::player::state::{
    CAPSULE_HEIGHT, GRAVITY, RUN_BACK_RATIO, RUN_SPEED, TERMINAL_VELOCITY, TURN_RATE, WALK_RATIO,
};
use crate::player::swim::{SWIM_BACK_SPEED, SWIM_SPEED};

/// A mover with none of these set is not stepped at all: it stands where its last move put it.
const INTEGRATED: u32 = 0xff | flags::FALLING;
/// A prediction this close to a waiting move's position, squared yards across, needs no blend.
const RECONCILE_TOL_SQ: f32 = 7.716e-4;
/// A turn smaller than this is not worth blending.
const FACING_DEAD_ZONE: f32 = 9.5367e-7;
/// A body turning in place by more than this a frame shuffles its feet.
const TURN_LATCH_BAND: f32 = 1.0e-5;

/// One move as the server relayed it, with the state it did not repeat filled in from before.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelayMove {
    /// The server's stamp: its tick, in milliseconds.
    pub wire_ms: u32,
    /// Feet, WoW coordinates.
    pub position: [f32; 3],
    pub orientation: f32,
    pub flags: u32,
    pub pitch: f32,
    pub fall_time: u32,
    pub jump: Option<Jump>,
}

#[derive(Clone, Debug)]
struct PendingMove {
    fire_ms: f64,
    mv: RelayMove,
}

/// Another player's pose as this window simulates it, WoW coordinates; its transform follows.
#[derive(Component, Clone, Debug)]
pub struct RemoteMotion {
    pub wow_pos: [f32; 3],
    pub orientation: f32,
    pub flags: u32,
    pub pitch: f32,
    /// Horizontal speed, or a swimmer's stroke speed, yd/s.
    pub speed: f32,
    /// Up positive, yd/s; 0 on the ground.
    pub vertical_velocity: f32,
    /// A jump's horizontal velocity, frozen at its launch.
    pub jump_xy_vel: [f32; 2],
    pending: VecDeque<PendingMove>,
    relay: RelayChain,
}

impl RemoteMotion {
    /// A player first seen doing `mv`, arriving at `now_ms`: the chain seeds on it and it
    /// applies at once.
    pub fn seeded(mv: &RelayMove, now_ms: f64) -> Self {
        let mut rm = Self {
            wow_pos: mv.position,
            orientation: mv.orientation,
            flags: 0,
            pitch: 0.0,
            speed: 0.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            pending: VecDeque::new(),
            relay: RelayChain::default(),
        };
        rm.relay.schedule(mv.wire_ms, now_ms, 0, true);
        rm.apply(mv);
        rm
    }

    /// Schedules a move that arrived at `now_ms`: it applies now if it is due and nothing waits
    /// before it, and waits otherwise.
    pub fn relayed(&mut self, mv: RelayMove, now_ms: f64) {
        let fire_ms = self
            .relay
            .schedule(mv.wire_ms, now_ms, self.flags, self.pending.is_empty());
        if self.pending.is_empty() && fire_ms <= now_ms {
            self.apply(&mv);
        } else {
            self.pending.push_back(PendingMove { fire_ms, mv });
        }
    }

    fn apply(&mut self, mv: &RelayMove) {
        let (vertical, xy) = jump_seed(mv.jump, mv.fall_time);
        self.wow_pos = mv.position;
        self.orientation = mv.orientation;
        self.flags = mv.flags;
        self.pitch = mv.pitch;
        self.vertical_velocity = vertical;
        self.jump_xy_vel = xy;
    }

    /// One frame of dead reckoning from the last applied state: the position, facing, vertical
    /// speed and horizontal speed `dt` later.
    fn advance(&self, dt: f32) -> ([f32; 3], f32, f32, f32) {
        if self.flags & flags::FALLING != 0 {
            let mut pos = self.wow_pos;
            pos[0] += self.jump_xy_vel[0] * dt;
            pos[1] += self.jump_xy_vel[1] * dt;
            let (vertical, mean_vy) = fall_step(self.vertical_velocity, dt, TERMINAL_VELOCITY);
            pos[2] += mean_vy * dt;
            let speed = self.jump_xy_vel[0].hypot(self.jump_xy_vel[1]);
            return (pos, self.orientation, vertical, speed);
        }
        let turn = f32::from(i8::from(self.flags & flags::TURN_LEFT != 0))
            - f32::from(i8::from(self.flags & flags::TURN_RIGHT != 0));
        let orientation = self.orientation + turn * TURN_RATE * dt;
        let (hp, vp) = if self.flags & flags::SWIMMING != 0 {
            (ops::cos(self.pitch), ops::sin(self.pitch))
        } else {
            (1.0, 0.0)
        };
        let (fwd, left) = (
            [ops::cos(orientation), ops::sin(orientation)],
            [-ops::sin(orientation), ops::cos(orientation)],
        );
        let axis = |plus: u32, minus: u32| {
            f32::from(i8::from(self.flags & plus != 0))
                - f32::from(i8::from(self.flags & minus != 0))
        };
        let fwd_amt = axis(flags::FORWARD, flags::BACKWARD);
        let left_amt = axis(flags::STRAFE_LEFT, flags::STRAFE_RIGHT);
        let d = [
            fwd_amt * fwd[0] * hp + left_amt * left[0],
            fwd_amt * fwd[1] * hp + left_amt * left[1],
            fwd_amt * vp,
        ];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let mut pos = self.wow_pos;
        let speed = if len > 1.0e-4 {
            let base = current_speed(self.flags);
            for (p, d) in pos.iter_mut().zip(d) {
                *p += d * base * dt / len;
            }
            base
        } else {
            0.0
        };
        (pos, orientation, 0.0, speed)
    }
}

/// The speed a mover's flags travel at, yd/s: a walk outranks a backpedal.
fn current_speed(f: u32) -> f32 {
    if f & flags::ANY_MOVE == 0 {
        0.0
    } else if f & flags::SWIMMING != 0 {
        if f & flags::BACKWARD != 0 {
            SWIM_BACK_SPEED.min(SWIM_SPEED)
        } else {
            SWIM_SPEED
        }
    } else if f & flags::WALK_MODE != 0 {
        RUN_SPEED * WALK_RATIO
    } else if f & flags::BACKWARD != 0 {
        RUN_SPEED * RUN_BACK_RATIO
    } else {
        RUN_SPEED
    }
}

/// The vertical speed, up positive, and the frozen horizontal velocity of an arc whose launch is
/// `jump` (its vertical speed down positive) and which has been airborne `fall_time` ms.
fn jump_seed(jump: Option<Jump>, fall_time: u32) -> (f32, [f32; 2]) {
    jump.map_or((0.0, [0.0; 2]), |j| {
        let t = fall_time as f32 / 1000.0;
        (
            (-j.z_speed - GRAVITY * t).max(-TERMINAL_VELOCITY),
            [j.cos * j.xy_speed, j.sin * j.xy_speed],
        )
    })
}

/// Blends `pos` toward a waiting move's `target` by this frame's share of the time left, when
/// the prediction at its fire time would miss it; height counts only for a swimmer.
fn reconcile_lerp(
    mut pos: [f32; 3],
    predicted: [f32; 3],
    target: [f32; 3],
    swimming: bool,
    dt: f32,
    remaining_s: f32,
) -> [f32; 3] {
    let d = [
        predicted[0] - target[0],
        predicted[1] - target[1],
        predicted[2] - target[2],
    ];
    let dist_sq = d[0] * d[0] + d[1] * d[1] + if swimming { d[2] * d[2] } else { 0.0 };
    if dist_sq < RECONCILE_TOL_SQ {
        return pos;
    }
    let f = dt / (dt + remaining_s);
    for (p, t) in pos.iter_mut().zip(target) {
        *p += (t - *p) * f;
    }
    pos
}

/// Turns `orientation` the short way toward a waiting move's `target`, landing on it at the fire
/// time.
fn facing_lerp(orientation: f32, target: f32, dt: f32, remaining_s: f32) -> f32 {
    let mut d = (target - orientation) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    if d.abs() < FACING_DEAD_ZONE {
        return orientation;
    }
    orientation + d * (dt / (dt + remaining_s))
}

/// A swimmer stroking forward or back is drawn pitched along its travel; otherwise level.
pub fn swim_body_rotation(yaw: f32, flags: u32, pitch: f32) -> Quat {
    if flags & flags::SWIMMING != 0 && flags & (flags::FORWARD | flags::BACKWARD) != 0 {
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
    } else {
        Quat::from_rotation_y(yaw)
    }
}

/// Applies every relayed move whose time has come.
pub(super) fn drain_pending_moves(
    time: Res<'_, Time<Real>>,
    mut q: Query<'_, '_, &mut RemoteMotion>,
) {
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    for mut rm in &mut q {
        while rm.pending.front().is_some_and(|p| p.fire_ms <= now_ms) {
            if let Some(p) = rm.pending.pop_front() {
                rm.apply(&p.mv);
            }
        }
    }
}

type Remotes<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut RemoteMotion,
        &'static mut UnitMotion,
        Option<&'static mut BodyTwist>,
    ),
>;

/// Steps every other player one frame: dead-reckoned from its flags, the step met by the world as
/// the player's own is, blended toward a waiting move, and drawn with its strafe and swim pose.
pub(super) fn extrapolate_remote_units(
    time: Res<'_, Time<Real>>,
    world: WorldCollision<'_, '_>,
    capsule: Res<'_, PlayerCapsule>,
    mut q: Remotes<'_, '_>,
) {
    let dt = time.delta_secs();
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    for (mut t, mut rm, mut motion, twist) in &mut q {
        let (mut pos, mut orientation, vertical_velocity, speed) = rm.advance(dt);
        let airborne = rm.flags & flags::FALLING != 0;
        let integrating = rm.flags & INTEGRATED != 0;
        if integrating && rm.flags & flags::SWIMMING == 0 {
            let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
            let from = wow_to_bevy(rm.wow_pos) + half_h;
            let vel = if dt > 1.0e-6 {
                (wow_to_bevy(pos) + half_h - from) / dt
            } else {
                Vec3::ZERO
            };
            let center = if airborne {
                airborne_step(&world, &capsule.0, from, vel, time.delta())
            } else {
                let g = grounded_step(
                    &world,
                    &capsule.0,
                    from,
                    vel,
                    time.delta(),
                    Support::default(),
                );
                match g.unsupported {
                    Some(_) => Vec3::new(g.center.x, from.y, g.center.z),
                    None => g.center,
                }
            };
            pos = bevy_to_wow(center - half_h);
        }
        if let Some(next) = rm.pending.front() {
            let remaining_s = ((next.fire_ms - now_ms) / 1000.0) as f32;
            if remaining_s > 0.0 {
                orientation = facing_lerp(orientation, next.mv.orientation, dt, remaining_s);
                let (predicted, ..) = rm.advance(dt + remaining_s);
                let swimming = next.mv.flags & flags::SWIMMING != 0;
                pos = reconcile_lerp(pos, predicted, next.mv.position, swimming, dt, remaining_s);
            }
        }
        let still = flags::ANY_MOVE | flags::TURNING | flags::FALLING | flags::SWIMMING;
        let step = if rm.flags & still == 0 {
            wrap_pi(orientation - rm.orientation)
        } else {
            0.0
        };
        rm.wow_pos = pos;
        rm.orientation = orientation;
        rm.vertical_velocity = vertical_velocity;
        rm.speed = speed;
        let shuffle = match step {
            s if s > TURN_LATCH_BAND => flags::TURN_LEFT,
            s if s < -TURN_LATCH_BAND => flags::TURN_RIGHT,
            _ => 0,
        };
        motion.set_if_neq(UnitMotion {
            speed,
            vertical_speed: vertical_velocity,
            flags: rm.flags | shuffle,
            ..*motion
        });
        let translation = wow_to_bevy(pos);
        if t.translation != translation {
            t.translation = translation;
        }
        let offset = if rm.flags & flags::SWIMMING != 0 {
            0.0
        } else {
            strafe_body_offset(rm.flags)
        };
        let yaw = if offset == 0.0 {
            orientation
        } else {
            let (current, ..) = t.rotation.to_euler(EulerRot::YXZ);
            ease_strafe_yaw(current, orientation, offset, dt)
        };
        let rotation = swim_body_rotation(yaw, rm.flags, rm.pitch);
        if t.rotation != rotation {
            t.rotation = rotation;
        }
        if let Some(mut twist) = twist {
            twist.yaw_gap = wrap_pi(orientation - yaw);
        }
    }
}

#[cfg(test)]
mod tests;
