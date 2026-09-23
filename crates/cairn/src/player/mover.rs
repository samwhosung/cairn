//! The walk and fall step: a kinematic capsule over the world's one-sided sweeps.

use std::time::Duration;

use avian3d::character_controller::move_and_slide::{
    MoveAndSlideConfig, MoveAndSlideHitResponse, MoveHitData,
};
use avian3d::prelude::*;
use bevy::prelude::*;
use world::collision::WorldCollision;

use super::state::{
    AIR_NUDGE_SPEED, CAPSULE_HEIGHT, FOOT_CONE_HEIGHT, GRAVITY, GROUND_COS, GROUND_PROBE,
    JUMP_SPEED, LAND_PROBE, Player, SKIN_WIDTH, STEP_SLOPE_RATIO, STEP_SNAP_SLACK,
    STEP_UP_ADVANCE_PER_YARD, STEP_UP_HEIGHT, TERMINAL_VELOCITY, WEDGE_MIN_FALL, WEDGE_STALL_RATIO,
    WEDGE_STILL_FRAMES,
};

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Outcome {
    pub settling: bool,
    /// On walkable ground (or wedged) and not rising.
    pub grounded: bool,
    pub jumped: bool,
    /// A standstill jump's one steer fired this frame.
    pub air_nudged: bool,
}

/// Advances the body one frame.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn step(
    player: &mut Player,
    time: &Time,
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    moving: bool,
    dir: Vec3,
    speed: f32,
    want_jump: bool,
) -> Outcome {
    let dt = time.delta_secs();
    let input_horiz = if moving {
        dir.normalize() * speed
    } else {
        Vec3::ZERO
    };
    let half_h = Vec3::Y * (CAPSULE_HEIGHT * 0.5);
    let mut center = player.pos + half_h;
    let probe_down =
        |c: Vec3, dist: f32| world.cast_body(capsule, c, Vec3::NEG_Y * dist, SKIN_WIDTH);

    let ground_reach = if player.airborne_since.is_some() {
        LAND_PROBE
    } else {
        GROUND_PROBE
    };
    let on_walkable = probe_down(center, ground_reach).is_some_and(|h| h.normal1.y >= GROUND_COS);
    let settling = player.settling;
    // A body part-way up a foot cone is standing: the probe sees only the riser it rides.
    let on_floor = !settling && (on_walkable || player.steep_support) && player.vel_y <= 0.0;
    if player.wedged && (on_floor || settling || probe_down(center, LAND_PROBE).is_none()) {
        player.wedged = false;
    }
    let grounded = on_floor || player.wedged;
    let stepped_off = !player.last_step_airborne;

    let mut jumped = false;
    if settling {
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
    } else if grounded {
        player.vel_y = 0.0;
        if want_jump {
            player.vel_y = JUMP_SPEED;
            player.launch_vz = JUMP_SPEED;
            player.wedged = false;
            player.steep_support = false;
            jumped = true;
            player.arc_steer_spent = moving;
        }
    } else if stepped_off {
        player.launch_vz = 0.0;
        player.arc_steer_spent = moving;
    }

    let mut fall_mean_vy = player.vel_y;
    if !settling && (jumped || !grounded) {
        let (end_vy, mean_vy) = fall_step(player.vel_y, dt, TERMINAL_VELOCITY);
        player.vel_y = end_vy;
        fall_mean_vy = mean_vy;
    }
    let mut air_nudged = false;
    if grounded {
        player.horiz_vel = input_horiz;
    } else if !settling && moving && !player.arc_steer_spent {
        player.horiz_vel = dir.normalize_or_zero() * AIR_NUDGE_SPEED;
        air_nudged = true;
        player.arc_steer_spent = true;
    }

    let pre_move = center;
    if !settling && grounded && !jumped {
        let g = grounded_step(
            world,
            capsule,
            center,
            player.horiz_vel,
            time.delta(),
            Support {
                rise: STEP_UP_HEIGHT,
                steep: player.steep_support,
            },
        );
        center = g.center - Vec3::Y * g.unsupported.unwrap_or(0.0);
        player.steep_support = g.steep_support;
    } else {
        let velocity = if settling {
            Vec3::ZERO
        } else {
            player.horiz_vel + Vec3::Y * fall_mean_vy
        };
        center = airborne_step(world, capsule, center, velocity, time.delta());
        player.steep_support = false;
    }
    // A fall achieving a sliver of what gravity intended, frame after frame, is held between
    // steep faces: it lands there.
    if !settling
        && !grounded
        && !jumped
        && player.vel_y < -WEDGE_MIN_FALL
        && (pre_move.y - center.y) < -player.vel_y * dt * WEDGE_STALL_RATIO
    {
        player.wedge_still += 1;
        if player.wedge_still >= WEDGE_STILL_FRAMES {
            player.wedged = true;
            player.wedge_still = 0;
            player.vel_y = 0.0;
        }
    } else {
        player.wedge_still = 0;
    }
    let grounded = grounded || player.wedged;
    player.pos = center - half_h;
    // A launch frame is the arc's first airborne frame, though the body still stands.
    player.last_step_airborne = !settling && (jumped || !grounded);
    Outcome {
        settling,
        grounded,
        jumped,
        air_nudged,
    }
}

/// What already holds the body up entering the frame.
#[derive(Clone, Copy)]
pub struct Support {
    /// The rise budget: how high the step-up may lift, and the yardstick of its reach.
    pub rise: f32,
    /// The support is a certified steep contact, which opens the step-down's deep reach.
    pub steep: bool,
}

impl Default for Support {
    fn default() -> Self {
        Self {
            rise: STEP_UP_HEIGHT,
            steep: false,
        }
    }
}

pub struct GroundedStep {
    pub center: Vec3,
    /// The walkable floor the snap settled onto.
    #[cfg_attr(not(test), allow(dead_code, reason = "the step-down's tests read it"))]
    pub ground: Option<Entity>,
    /// Height the climb gained: a ride's, or a pop's.
    #[cfg_attr(not(test), allow(dead_code, reason = "the climb's tests read it"))]
    pub climb: Option<f32>,
    /// Held up by a certified steep contact rather than a walkable floor.
    pub steep_support: bool,
    /// The fall's opening drop when no floor was in sight at all; not applied to `center`.
    pub unsupported: Option<f32>,
}

/// Step-up, slide, step-down, from a capsule centre and this frame's horizontal velocity.
pub fn grounded_step(
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    center: Vec3,
    horiz_vel: Vec3,
    dt: Duration,
    support: Support,
) -> GroundedStep {
    let cast = |from: Vec3, disp: Vec3| world.cast_body(capsule, from, disp, SKIN_WIDTH);
    let speed = horiz_vel.length();
    let attempt = if speed > 1.0e-6 {
        let travel = speed * dt.as_secs_f32();
        let body_reach = travel.max(support.rise * STEP_UP_ADVANCE_PER_YARD);
        step_up(
            &cast,
            center,
            horiz_vel / speed,
            travel,
            body_reach,
            support.rise,
        )
    } else {
        StepAttempt {
            contact: None,
            verdict: StepVerdict::NoFace,
        }
    };
    let ride_to = match (attempt.verdict, attempt.contact) {
        (StepVerdict::Commit { landed, .. }, Some((p, _)))
            if p.y - (center.y - CAPSULE_HEIGHT * 0.5) <= FOOT_CONE_HEIGHT =>
        {
            Some(landed.y)
        }
        _ => None,
    };
    let popped = match attempt.verdict {
        StepVerdict::Commit { up, .. } if ride_to.is_none() => Some(up),
        _ => None,
    };
    let start = center + Vec3::Y * popped.unwrap_or(0.0);
    let mut rode = false;
    let out = world.slide_body(
        capsule,
        start,
        horiz_vel,
        dt,
        &MoveAndSlideConfig::default(),
        |hit| {
            if let Some(ride) = walkable_ride_velocity(**hit.normal, *hit.velocity) {
                *hit.velocity = ride;
                return MoveAndSlideHitResponse::Accept;
            }
            if ride_to.is_some_and(|ceiling| hit.position.y < ceiling)
                && let Some(up) = foot_cone_ride(**hit.normal, *hit.velocity)
            {
                *hit.velocity = up;
                rode = true;
                return MoveAndSlideHitResponse::Accept;
            }
            if let Some(followed) = steep_contact_shear(**hit.normal, *hit.velocity) {
                *hit.velocity = followed;
            }
            MoveAndSlideHitResponse::Accept
        },
    );
    let mut slid = out.position;

    let d = slid - start;
    let cone_reach = d.x.hypot(d.z) * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    let reach = cone_reach
        + if support.steep || popped.is_some() {
            support.rise
        } else {
            0.0
        };
    let floor = cast(slid, Vec3::NEG_Y * reach).map(|h| (h.distance, h.normal1.y, h.entity));
    let mut ground = None;
    let mut steep_support = false;
    let mut unsupported = None;
    // A ride's height is earned: only a floor above where the ride began may end it.
    let ride_rise = (slid.y - start.y).max(0.0);
    if let Some((distance, normal_y, entity)) = floor {
        let drop = distance.max(0.0);
        let walkable = normal_y >= GROUND_COS;
        if !(rode && walkable && drop >= ride_rise) {
            // A capsule hangs clear of an edge a cone would ride down, so it descends at most a
            // cone's worth a frame.
            slid.y -= drop.min(cone_reach);
            if drop > cone_reach {
                steep_support = true;
            } else if walkable {
                ground = Some(entity);
            } else {
                // Support is earned by descending: a motionless body must slide off a bank.
                steep_support = drop > STEP_SNAP_SLACK;
            }
        }
    } else if !rode {
        unsupported = Some(reach.max(0.0).min(cone_reach));
    }
    GroundedStep {
        center: slid,
        climb: (rode || popped.is_some()).then_some(slid.y - center.y),
        steep_support: ((rode || popped.is_some()) && ground.is_none()) || steep_support,
        ground,
        unsupported,
    }
}

/// One frame of the fall, exact under constant acceleration: the end-of-step vertical speed and
/// the mean speed to move at. A step that reaches terminal partway accelerates, then coasts.
#[allow(clippy::manual_midpoint)]
pub fn fall_step(v0: f32, dt: f32, terminal: f32) -> (f32, f32) {
    let unclamped = v0 - GRAVITY * dt;
    if unclamped >= -terminal {
        return (unclamped, 0.5 * (v0 + unclamped));
    }
    if v0 <= -terminal {
        return (-terminal, -terminal);
    }
    let t_c = (v0 + terminal) / GRAVITY;
    let displacement = 0.5 * (v0 - terminal) * t_c - terminal * (dt - t_c);
    (-terminal, displacement / dt)
}

/// One airborne step: the arc's slide, and nothing may lift it.
pub fn airborne_step(
    world: &WorldCollision<'_, '_>,
    capsule: &Collider,
    center: Vec3,
    velocity: Vec3,
    dt: Duration,
) -> Vec3 {
    world
        .slide_body(
            capsule,
            center,
            velocity,
            dt,
            &MoveAndSlideConfig::default(),
            |hit| {
                if let Some(followed) = steep_contact_shear(**hit.normal, *hit.velocity) {
                    *hit.velocity = followed;
                }
                MoveAndSlideHitResponse::Accept
            },
        )
        .position
}

/// An axis-aligned quad's normal can come out at `y = ±0`; a real overhang is far below this.
const OVERHANG_EPS: f32 = -1.0e-4;

/// Too steep to stand on, not a ceiling.
fn is_steep_face(ny: f32) -> bool {
    (OVERHANG_EPS..GROUND_COS).contains(&ny)
}

/// Against an opposing walkable face, keep the horizontal velocity exactly and ride the plane.
fn walkable_ride_velocity(n: Vec3, v: Vec3) -> Option<Vec3> {
    if n.y < GROUND_COS || v.dot(n) >= 0.0 {
        return None;
    }
    Some(Vec3::new(v.x, -(v.x * n.x + v.z * n.z) / n.y, v.z))
}

/// What the foot cone's skirt would do against a steep opposing face: rise at the cone's slope
/// times the closing speed, horizontal untouched. Whether the body may climb is the caller's.
fn foot_cone_ride(n: Vec3, v: Vec3) -> Option<Vec3> {
    if !is_steep_face(n.y) {
        return None;
    }
    let h = Vec3::new(n.x, 0.0, n.z).normalize();
    let into = -(v.x * h.x + v.z * h.z);
    if into <= 0.0 {
        return None;
    }
    Some(Vec3::new(v.x, into * STEP_SLOPE_RATIO, v.z))
}

/// A steep face pushes out along its horizontal normal only: the residual lies in the plane, the
/// descent passes untouched, and no contact can add upward motion.
fn steep_contact_shear(n: Vec3, v: Vec3) -> Option<Vec3> {
    if !is_steep_face(n.y) {
        return None;
    }
    let vn = v.dot(n);
    if vn >= 0.0 {
        return None;
    }
    let h = Vec3::new(n.x, 0.0, n.z);
    Some(v - (vn / h.length_squared()) * h)
}

#[derive(Clone, Copy, Debug)]
pub enum StepVerdict {
    /// Nothing steep and opposing within the look-ahead.
    NoFace,
    NoHeadroom,
    NoFloor,
    SteepFloor,
    /// No height gained: a graze, a wall, a gap between trunks. The slide owns the frame.
    NetZero,
    /// The obstacle can be cleared. `landed` is where the settle found floor, a full reach ahead.
    Commit {
        landed: Vec3,
        up: f32,
    },
}

pub struct StepAttempt {
    /// The opposing face's contact point and normal.
    pub contact: Option<(Vec3, Vec3)>,
    pub verdict: StepVerdict,
}

/// Rise by the free headroom up to `rise`, advance at that height, settle onto a walkable floor
/// higher than the feet; certify only a real gain.
pub fn step_up(
    cast: &impl Fn(Vec3, Vec3) -> Option<MoveHitData>,
    center: Vec3,
    dir_h: Vec3,
    look: f32,
    advance: f32,
    rise: f32,
) -> StepAttempt {
    let Some(ahead) = cast(center, dir_h * look) else {
        return StepAttempt {
            contact: None,
            verdict: StepVerdict::NoFace,
        };
    };
    let n = ahead.normal1;
    if n.y >= GROUND_COS || n.y < OVERHANG_EPS || n.dot(dir_h) >= 0.0 {
        return StepAttempt {
            contact: None,
            verdict: StepVerdict::NoFace,
        };
    }
    let at = |verdict| StepAttempt {
        contact: Some((ahead.point1, n)),
        verdict,
    };
    let up = cast(center, Vec3::Y * rise).map_or(rise, |h| h.distance);
    if up < 1e-3 {
        return at(StepVerdict::NoHeadroom);
    }
    let raised = center + Vec3::Y * up;
    let fwd = cast(raised, dir_h * advance).map_or(advance, |h| h.distance);
    let over = raised + dir_h * fwd;
    let reach = up + advance * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    let Some(down) = cast(over, Vec3::NEG_Y * reach) else {
        return at(StepVerdict::NoFloor);
    };
    if down.normal1.y < GROUND_COS {
        return at(StepVerdict::SteepFloor);
    }
    let landed = over + Vec3::NEG_Y * down.distance;
    if landed.y - center.y <= 0.05 {
        return at(StepVerdict::NetZero);
    }
    at(StepVerdict::Commit { landed, up })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod walks;
