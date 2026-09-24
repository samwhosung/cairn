//! Swimming: deep enough water lifts the body off the floor, and it swims in 3-D along its pitched
//! facing.

use std::time::Duration;

use avian3d::character_controller::move_and_slide::{MoveAndSlideConfig, MoveAndSlideHitResponse};
use avian3d::prelude::*;
use bevy::prelude::*;
use world::collision::{Liquids, WorldCollision};
use world::coords::bevy_to_wow;

use super::input::MoveAxes;
use super::mover::Outcome;
use super::state::{CAPSULE_HEIGHT, GRAVITY, GROUND_COS, GROUND_PROBE, Player, SKIN_WIDTH};

/// Swim speed, yd/s.
pub const SWIM_SPEED: f32 = 4.722_222;
/// Backward swim speed, yd/s.
pub const SWIM_BACK_SPEED: f32 = 2.5;
/// The take-off speed of a jump out of the water, yd/s.
pub const SWIM_JUMP_SPEED: f32 = 9.096_748;
const SWIM_DEPTH_FRAC: f32 = 0.75;
/// The enter/leave band, yd, the same for every body height.
const SWIM_HYSTERESIS: f32 = 1.0 / 36.0;

/// Water deeper than this over the feet starts a swim; it is also the deepest a body wades.
pub fn swim_enter_depth(h: f32) -> f32 {
    SWIM_DEPTH_FRAC * h
}

/// Water shallower than this over the feet ends a swim.
pub fn swim_exit_depth(h: f32) -> f32 {
    swim_enter_depth(h) - SWIM_HYSTERESIS
}

/// How far under the surface a rising swimmer's feet stop: three-quarters submerged.
fn rest_cap(h: f32) -> f32 {
    swim_enter_depth(h)
}

/// The liquid surface over the feet, Bevy Y. Any liquid: lava and slime are swum too.
pub fn surface_over_feet(liquids: &Liquids<'_, '_>, feet: Vec3) -> Option<f32> {
    let wow = bevy_to_wow(feet);
    liquids
        .liquid_at(wow)
        .map(|hit| feet.y + (hit.surface_z - wow[2]))
}

/// Latches swimming from the depth over the feet, with the enter/leave band. A fresh launch is
/// not re-latched until its upward speed has fallen to half, so a hop out of deep water tops out
/// and then swims on.
pub fn update_swimming(player: &mut Player, surface_y: Option<f32>, now: f32) -> bool {
    let was = player.swimming;
    let Some(surface) = surface_y else {
        player.swimming = false;
        stop_pitch(player, was);
        return false;
    };
    let depth = surface - player.pos.y;
    let h = player.collision_height;
    player.swimming = if player.swimming {
        depth >= swim_exit_depth(h)
    } else {
        let hop_blocked = player.vel_y > 0.0
            && player
                .airborne_since
                .is_some_and(|t0| now - t0 < player.jump_zspeed / (2.0 * GRAVITY));
        depth > swim_enter_depth(h) && !hop_blocked
    };
    stop_pitch(player, was);
    player.swimming
}

/// Leaving the water by depth levels the mover's pitch; a jump out keeps it.
fn stop_pitch(player: &mut Player, was: bool) {
    if was && !player.swimming {
        player.mover_pitch = 0.0;
    }
}

/// The take-off frame of a jump out of the water; the caller has already dropped the swim latch.
pub fn breach_step(
    player: &mut Player,
    dt: Duration,
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
) -> Outcome {
    player.vel_y = SWIM_JUMP_SPEED;
    player.launch_vz = SWIM_JUMP_SPEED;
    player.arc_steer_spent = player.horiz_vel.length_squared() > 0.0;
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let out = world.slide_body(
        capsule,
        player.pos + half_h,
        player.horiz_vel + Vec3::Y * player.vel_y,
        dt,
        &MoveAndSlideConfig::default(),
        |_hit| MoveAndSlideHitResponse::Accept,
    );
    player.pos = out.position - half_h;
    Outcome {
        settling: false,
        grounded: false,
        jumped: true,
        air_nudged: false,
    }
}

pub struct SwimOutcome {
    /// Walkable floor right under the feet: a swim into the shallows resolves onto it.
    pub grounded: bool,
    /// The travel pitch after the surface redirect, when it bit.
    pub surface_pitch: Option<f32>,
}

/// Caps a rising stroke's vertical at `cap` and turns the rest of its speed level.
fn cap_redirect(input_vel: Vec3, cap: f32) -> (Vec3, Option<f32>) {
    if input_vel.y <= 0.0 || input_vel.y <= cap {
        return (input_vel, None);
    }
    let speed = input_vel.length();
    let level_dir = Vec3::new(input_vel.x, 0.0, input_vel.z).normalize_or_zero();
    let level_speed = (speed * speed - cap * cap).max(0.0).sqrt();
    (
        level_dir * level_speed + Vec3::Y * cap,
        Some(cap.atan2(level_speed)),
    )
}

/// How far the feet must sink to be back on the rest line; never an upward pull.
fn settle_to_rest(feet_y: f32, surface_y: f32, h: f32) -> f32 {
    (feet_y - (surface_y - rest_cap(h))).max(0.0)
}

/// One swim frame. A stroking swimmer left above the rest line (a river's surface fell away) is
/// swept back onto it; an idle floater's depth is frozen.
pub fn swim_step(
    player: &mut Player,
    time: &Time,
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    input_vel: Vec3,
    surface: f32,
    surface_at: impl Fn(Vec3) -> Option<f32>,
) -> SwimOutcome {
    let dt = time.delta_secs();
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let center = player.pos + half_h;
    let cap = if dt > 0.0 {
        let rest_feet_y = surface - rest_cap(player.collision_height);
        ((rest_feet_y - player.pos.y) / dt).max(0.0)
    } else {
        f32::INFINITY
    };
    let (vel, surface_pitch) = cap_redirect(input_vel, cap);
    let out = world.slide_body(
        capsule,
        center,
        vel,
        time.delta(),
        &MoveAndSlideConfig::default(),
        |_hit| MoveAndSlideHitResponse::Accept,
    );
    let mut c = out.position;
    if input_vel != Vec3::ZERO
        && let Some(surface_now) = surface_at(c - half_h)
    {
        let excess = settle_to_rest(c.y - half_h.y, surface_now, player.collision_height);
        if excess > 0.0 {
            let drop = world
                .cast_body(capsule, c, Vec3::NEG_Y * excess, SKIN_WIDTH)
                .map_or(excess, |h| h.distance.min(excess));
            c.y -= drop;
        }
    }
    player.pos = c - half_h;
    player.vel_y = 0.0;
    player.horiz_vel = Vec3::new(vel.x, 0.0, vel.z);
    let grounded = world
        .cast_body(capsule, c, Vec3::NEG_Y * GROUND_PROBE, SKIN_WIDTH)
        .is_some_and(|h| h.normal1.y >= GROUND_COS);
    SwimOutcome {
        grounded,
        surface_pitch,
    }
}

/// Returns the outcome and the pitch the body presents.
#[allow(clippy::too_many_arguments)]
pub fn drive_step(
    player: &mut Player,
    time: &Time,
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    liquids: &Liquids<'_, '_>,
    surface_y: Option<f32>,
    (forward, right): (Vec3, Vec3),
    (swim_fwd, swim_side): (f32, f32),
) -> (Outcome, f32) {
    let (sp, cp) = ops::sin_cos(player.mover_pitch);
    let v = (forward * cp + Vec3::Y * sp) * swim_fwd + right * swim_side;
    let dir = v.normalize_or_zero();
    let speed = if swim_fwd < 0.0 {
        SWIM_BACK_SPEED.min(SWIM_SPEED)
    } else {
        SWIM_SPEED
    };
    player.swim_stroke_speed = if dir == Vec3::ZERO { 0.0 } else { speed };
    // A momentary miss of the surface sample holds the swimmer at its own depth for the frame.
    let surface = surface_y.unwrap_or(player.pos.y);
    let out = swim_step(player, time, world, capsule, dir * speed, surface, |feet| {
        surface_over_feet(liquids, feet)
    });
    let outcome = Outcome {
        settling: false,
        grounded: out.grounded,
        jumped: false,
        air_nudged: false,
    };
    (outcome, out.surface_pitch.unwrap_or(player.mover_pitch))
}

pub fn translate_amounts(axes: &MoveAxes) -> (f32, f32) {
    let mut side = 0.0_f32;
    if axes.strafe_right {
        side += 1.0;
    }
    if axes.strafe_left {
        side -= 1.0;
    }
    if axes.mouselook {
        if axes.turn_right {
            side += 1.0;
        }
        if axes.turn_left {
            side -= 1.0;
        }
    }
    (axes.fwd.signum() as f32, side)
}

#[cfg(test)]
mod tests;
