//! Another player's motion between the moves the server relays.

use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::time::Duration;

use avian3d::prelude::Collider;
use bevy::math::ops;
use bevy::prelude::*;
use bevy::time::Real;
use protocol::{Jump, flags};
use world::collision::WorldCollision;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{BodyTwist, UnitMotion};

use super::relay::ReplayTiming;
use crate::player::PlayerCapsule;
use crate::player::gait::{ease_strafe_yaw, strafe_body_offset, wrap_pi};
use crate::player::mover::{Support, airborne_step, fall_step, grounded_step};
use crate::player::state::{
    CAPSULE_HEIGHT, GRAVITY, RUN_BACK_RATIO, RUN_SPEED, TERMINAL_VELOCITY, TURN_RATE, WALK_RATIO,
};
use crate::player::swim::{SWIM_BACK_SPEED, SWIM_SPEED};

const RECONCILE_TOL_SQ: f32 = 7.716e-4;
const FACING_DEAD_ZONE: f32 = 9.5367e-7;
const TURN_IN_PLACE_MIN: f32 = 1.0e-5;

/// One move as the server relayed it, with the state it did not repeat filled in from before.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelayMove {
    pub server_ms: u32,
    pub wow_pos: [f32; 3],
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
    /// Up positive, yd/s; 0 unless falling.
    pub vertical_velocity: f32,
    pub jump_xy_vel: [f32; 2],
    pending: VecDeque<PendingMove>,
    timing: ReplayTiming,
}

#[derive(Clone, Copy, Debug)]
struct Reckoned {
    wow_pos: [f32; 3],
    facing: f32,
    vertical_velocity: f32,
    speed: f32,
}

struct Airborne {
    vertical_velocity: f32,
    xy_velocity: [f32; 2],
}

impl RemoteMotion {
    pub fn seeded(mv: &RelayMove, arrived_ms: f64) -> Self {
        let mut rm = Self {
            wow_pos: mv.wow_pos,
            orientation: mv.orientation,
            flags: 0,
            pitch: 0.0,
            vertical_velocity: 0.0,
            jump_xy_vel: [0.0; 2],
            pending: VecDeque::new(),
            timing: ReplayTiming::default(),
        };
        rm.timing.schedule(mv.server_ms, arrived_ms, 0, true);
        rm.apply(mv);
        rm
    }

    pub fn relayed(&mut self, mv: RelayMove, arrived_ms: f64, now_ms: f64) {
        let fire_ms = self.timing.schedule(
            mv.server_ms,
            arrived_ms,
            self.flags,
            self.pending.is_empty(),
        );
        if self.pending.is_empty() && fire_ms <= now_ms {
            self.apply(&mv);
        } else {
            self.pending.push_back(PendingMove { fire_ms, mv });
        }
    }

    fn apply(&mut self, mv: &RelayMove) {
        let arc = airborne(mv.jump, mv.fall_time);
        self.wow_pos = mv.wow_pos;
        self.orientation = mv.orientation;
        self.flags = mv.flags;
        self.pitch = mv.pitch;
        self.vertical_velocity = arc.vertical_velocity;
        self.jump_xy_vel = arc.xy_velocity;
    }

    fn reckon(&self, dt: f32) -> Reckoned {
        if self.flags & flags::FALLING != 0 {
            let mut wow_pos = self.wow_pos;
            wow_pos[0] += self.jump_xy_vel[0] * dt;
            wow_pos[1] += self.jump_xy_vel[1] * dt;
            let (vertical_velocity, mean_vy) =
                fall_step(self.vertical_velocity, dt, TERMINAL_VELOCITY);
            wow_pos[2] += mean_vy * dt;
            return Reckoned {
                wow_pos,
                facing: self.orientation,
                vertical_velocity,
                speed: self.jump_xy_vel[0].hypot(self.jump_xy_vel[1]),
            };
        }
        let axis = |plus: u32, minus: u32| {
            f32::from(i8::from(self.flags & plus != 0))
                - f32::from(i8::from(self.flags & minus != 0))
        };
        let facing = self.orientation + axis(flags::TURN_LEFT, flags::TURN_RIGHT) * TURN_RATE * dt;
        let (hp, vp) = if self.flags & flags::SWIMMING != 0 {
            (ops::cos(self.pitch), ops::sin(self.pitch))
        } else {
            (1.0, 0.0)
        };
        let (fwd, left) = (
            [ops::cos(facing), ops::sin(facing)],
            [-ops::sin(facing), ops::cos(facing)],
        );
        let fwd_amt = axis(flags::FORWARD, flags::BACKWARD);
        let left_amt = axis(flags::STRAFE_LEFT, flags::STRAFE_RIGHT);
        let d = [
            fwd_amt * fwd[0] * hp + left_amt * left[0],
            fwd_amt * fwd[1] * hp + left_amt * left[1],
            fwd_amt * vp,
        ];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let mut wow_pos = self.wow_pos;
        let speed = if len > 1.0e-4 {
            let base = current_speed(self.flags);
            for (p, d) in wow_pos.iter_mut().zip(d) {
                *p += d * base * dt / len;
            }
            base
        } else {
            0.0
        };
        Reckoned {
            wow_pos,
            facing,
            vertical_velocity: 0.0,
            speed,
        }
    }
}

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

fn airborne(jump: Option<Jump>, fall_time: u32) -> Airborne {
    jump.map_or(
        Airborne {
            vertical_velocity: 0.0,
            xy_velocity: [0.0; 2],
        },
        |j| {
            let t = fall_time as f32 / 1000.0;
            Airborne {
                vertical_velocity: (-j.z_speed - GRAVITY * t).max(-TERMINAL_VELOCITY),
                xy_velocity: [j.cos * j.xy_speed, j.sin * j.xy_speed],
            }
        },
    )
}

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

pub fn swim_body_rotation(yaw: f32, flags: u32, pitch: f32) -> Quat {
    if flags & flags::SWIMMING != 0 && flags & (flags::FORWARD | flags::BACKWARD) != 0 {
        Quat::from_rotation_y(yaw) * Quat::from_rotation_x(pitch)
    } else {
        Quat::from_rotation_y(yaw)
    }
}

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

fn moves_through_the_world(f: u32) -> bool {
    f & flags::UNDER_WAY != 0 && f & flags::SWIMMING == 0
}

fn through_world(
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    rm: &RemoteMotion,
    to: [f32; 3],
    dt: Duration,
) -> [f32; 3] {
    if !moves_through_the_world(rm.flags) {
        return to;
    }
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let from = wow_to_bevy(rm.wow_pos) + half_h;
    let vel = if dt.as_secs_f32() > 1.0e-6 {
        (wow_to_bevy(to) + half_h - from) / dt.as_secs_f32()
    } else {
        Vec3::ZERO
    };
    let center = if rm.flags & flags::FALLING != 0 {
        airborne_step(world, capsule, from, vel, dt)
    } else {
        let g = grounded_step(world, capsule, from, vel, dt, Support::default());
        match g.unsupported {
            Some(_) => Vec3::new(g.center.x, from.y, g.center.z),
            None => g.center,
        }
    };
    bevy_to_wow(center - half_h)
}

fn toward_waiting(rm: &RemoteMotion, step: Reckoned, dt: f32, now_ms: f64) -> Reckoned {
    let Some(next) = rm.pending.front() else {
        return step;
    };
    let remaining_s = ((next.fire_ms - now_ms) / 1000.0) as f32;
    if remaining_s <= 0.0 {
        return step;
    }
    let predicted = rm.reckon(dt + remaining_s).wow_pos;
    let swimming = next.mv.flags & flags::SWIMMING != 0;
    Reckoned {
        wow_pos: reconcile_lerp(
            step.wow_pos,
            predicted,
            next.mv.wow_pos,
            swimming,
            dt,
            remaining_s,
        ),
        facing: facing_lerp(step.facing, next.mv.orientation, dt, remaining_s),
        ..step
    }
}

fn turn_in_place_flags(rm: &RemoteMotion, facing: f32) -> u32 {
    let still = flags::ANY_MOVE | flags::TURNING | flags::FALLING | flags::SWIMMING;
    let turned = if rm.flags & still == 0 {
        wrap_pi(facing - rm.orientation)
    } else {
        0.0
    };
    match turned {
        t if t > TURN_IN_PLACE_MIN => flags::TURN_LEFT,
        t if t < -TURN_IN_PLACE_MIN => flags::TURN_RIGHT,
        _ => 0,
    }
}

fn draw(t: &mut Transform, rm: &RemoteMotion, twist: Option<Mut<'_, BodyTwist>>, dt: f32) {
    let translation = wow_to_bevy(rm.wow_pos);
    if t.translation != translation {
        t.translation = translation;
    }
    let offset = if rm.flags & flags::SWIMMING != 0 {
        0.0
    } else {
        strafe_body_offset(rm.flags)
    };
    let yaw = if offset == 0.0 {
        rm.orientation
    } else {
        let (current, ..) = t.rotation.to_euler(EulerRot::YXZ);
        ease_strafe_yaw(current, rm.orientation, offset, dt)
    };
    let rotation = swim_body_rotation(yaw, rm.flags, rm.pitch);
    if t.rotation != rotation {
        t.rotation = rotation;
    }
    if let Some(mut twist) = twist {
        twist.yaw_gap = wrap_pi(rm.orientation - yaw);
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

pub(super) fn extrapolate_remote_units(
    time: Res<'_, Time<Real>>,
    world: WorldCollision<'_, '_>,
    capsule: Res<'_, PlayerCapsule>,
    mut q: Remotes<'_, '_>,
) {
    let dt = time.delta_secs();
    let now_ms = time.elapsed_secs_f64() * 1000.0;
    q.par_iter_mut()
        .for_each(|(mut t, mut rm, mut motion, twist)| {
            let mut step = rm.reckon(dt);
            step.wow_pos = through_world(&world, &capsule.0, &rm, step.wow_pos, time.delta());
            let step = toward_waiting(&rm, step, dt, now_ms);
            let in_place = turn_in_place_flags(&rm, step.facing);
            rm.wow_pos = step.wow_pos;
            rm.orientation = step.facing;
            rm.vertical_velocity = step.vertical_velocity;
            motion.set_if_neq(UnitMotion {
                speed: step.speed,
                vertical_speed: step.vertical_velocity,
                flags: rm.flags | in_place,
                ..*motion
            });
            draw(&mut t, &rm, twist, dt);
        });
}

#[cfg(test)]
mod tests;
