//! The avatar's state and the movement constants the client's mover runs on.

use bevy::prelude::*;

// Yards, seconds and radians throughout.
pub const RUN_SPEED: f32 = 7.0;
pub const WALK_RATIO: f32 = 2.5 / 7.0;
pub const RUN_BACK_RATIO: f32 = 4.5 / 7.0;
pub const TURN_RATE: f32 = std::f32::consts::PI;
/// The turn rate's scale while translating or falling.
pub const TURN_RATE_MOVING: f32 = 0.75;
/// The mouse-look clamp on the mover's pitch: ±89°.
pub const MOUSELOOK_PITCH_CLAMP: f32 = 1.553_343;
/// A standing body catches up with its aim at this multiple of the turn rate once steering stops.
pub const STATIONARY_CHASE_RATE: f32 = 8.0;

pub const CAPSULE_RADIUS: f32 = 1.0 / 3.0;
pub const CAPSULE_HEIGHT: f32 = 2.027_777_7;
/// The collision height the swim depths are fractions of, for a body without a model.
pub const DEFAULT_COLLISION_HEIGHT: f32 = 2.027_777_7;
pub const GRAVITY: f32 = 19.291_105;
pub const JUMP_SPEED: f32 = 7.955_547;
pub const TERMINAL_VELOCITY: f32 = 60.148_003;
/// A surface is walkable iff its normal is within 50° of up.
pub const GROUND_COS: f32 = 0.642_788;
/// How far below the feet counts as standing on ground while walking, yd.
pub const GROUND_PROBE: f32 = 0.2;
/// …and while airborne, so an arc ends where the slide actually touches the floor.
pub const LAND_PROBE: f32 = 0.05;
/// The step-vs-fall election's slope: the snap reaches `travel · ratio` below the feet (61.6°).
/// Also the foot cone's own slope.
pub const STEP_SLOPE_RATIO: f32 = 1.849_399;
/// The election's fixed slack, yd.
pub const STEP_SNAP_SLACK: f32 = 0.027_777_8;
/// The tallest obstacle the step-up lifts a player's body onto, yd.
pub const STEP_UP_HEIGHT: f32 = 1.0;
/// How far ahead the step-up looks for the tread it would stand on: `max(H·tan50°, r + 1/720)`.
pub const STEP_UP_ADVANCE: f32 = 1.191_753_6;
pub const STEP_UP_ADVANCE_PER_YARD: f32 = STEP_UP_ADVANCE / STEP_UP_HEIGHT;
/// Below this height above the feet a blocking edge meets the solid's slanted skirt and is ridden
/// up; above it the edge meets the vertical box and the step-up pops.
pub const FOOT_CONE_HEIGHT: f32 = CAPSULE_RADIUS * STEP_SLOPE_RATIO;
/// Consecutive stalled airborne frames that make a wedged fall a landing.
pub const WEDGE_STILL_FRAMES: u8 = 3;
/// A frame is stalled when its descent is under this fraction of what gravity intended…
pub const WEDGE_STALL_RATIO: f32 = 0.15;
/// …while already falling faster than this, yd/s.
pub const WEDGE_MIN_FALL: f32 = 1.0;
/// A standstill jump's one steer, yd/s.
pub const AIR_NUDGE_SPEED: f32 = 2.5;
/// A jump that descends this far below its launch becomes a far fall, yd.
pub const FALL_FAR_DROP: f32 = 0.111_11;
/// A step-off becomes a far fall after this long airborne, s.
pub const FALL_FAR_TIME: f32 = 0.5;
/// Kept between the capsule and what it hits, yd.
pub const SKIN_WIDTH: f32 = 0.02;

/// `pos` is the feet, in Bevy space.
#[derive(Resource, Clone, Debug, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Player {
    pub pos: Vec3,
    /// Vertical velocity, yd/s, +up.
    pub vel_y: f32,
    /// Horizontal velocity: from input on the ground, frozen take-off momentum in the air.
    pub horiz_vel: Vec3,
    /// The aim: what WASD moves along and the mouse's right-drag writes.
    pub face_yaw: f32,
    pub model_yaw: f32,
    /// The mover's pitch: steers a swimmer, held when nothing steers it.
    pub mover_pitch: f32,
    /// The camera pitch the last mouse-look push carried; the push fires when the aim moves.
    pub aim_pitch_seen: f32,
    pub autorun: bool,
    pub walking: bool,
    pub swimming: bool,
    /// Held in place, gravity off, until the collision around the body has arrived.
    pub settling: bool,
    pub airborne_since: Option<f32>,
    pub last_step_airborne: bool,
    /// Resting between steep faces: standing, though nothing walkable is under the feet.
    pub wedged: bool,
    pub wedge_still: u8,
    /// Held up by a certified steep contact: riding a low edge's skirt, or stepping down off one.
    pub steep_support: bool,
    /// The arc's launch vertical speed: the jump speed, or 0 for a step-off.
    pub jump_zspeed: f32,
    /// The launch speed recorded the instant a take-off is decided, before gravity touches it.
    pub launch_vz: f32,
    pub arc_steer_spent: bool,
    pub fall_start_y: f32,
    pub fall_far: bool,
    pub airborne_dirs: u32,
    pub move_flags: u32,
    pub collision_height: f32,
    /// The liquid surface over the feet as the last step left it, Bevy Y.
    pub liquid_surface: Option<f32>,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            pos: Vec3::ZERO,
            vel_y: 0.0,
            horiz_vel: Vec3::ZERO,
            face_yaw: 0.0,
            model_yaw: 0.0,
            mover_pitch: 0.0,
            aim_pitch_seen: 0.0,
            autorun: false,
            walking: false,
            swimming: false,
            settling: false,
            airborne_since: None,
            last_step_airborne: false,
            wedged: false,
            wedge_still: 0,
            steep_support: false,
            jump_zspeed: 0.0,
            launch_vz: 0.0,
            arc_steer_spent: false,
            fall_start_y: 0.0,
            fall_far: false,
            airborne_dirs: 0,
            move_flags: 0,
            collision_height: DEFAULT_COLLISION_HEIGHT,
            liquid_surface: None,
        }
    }
}

#[allow(clippy::fn_params_excessive_bools)]
pub fn forward_axis(forward: bool, backward: bool, both_buttons: bool, autorun: bool) -> i32 {
    i32::from(forward) + i32::from(both_buttons) + i32::from(autorun) - i32::from(backward)
}

pub fn autorun_cancelled(fwd_down: bool, back_down: bool, both_engaged: bool) -> bool {
    fwd_down || back_down || both_engaged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_axis_reproduces_the_autorun_state_table() {
        assert_eq!(forward_axis(false, false, false, true), 1);
        assert_eq!(forward_axis(true, false, false, false), 1);
        assert_eq!(forward_axis(true, false, false, true), 2);
        assert_eq!(forward_axis(false, true, false, false), -1);
        assert_eq!(forward_axis(false, true, false, true), 0);
    }

    #[test]
    fn the_two_orders_differ_and_only_one_resumes() {
        let mut autorun = true;
        if autorun_cancelled(false, true, false) {
            autorun = false;
        }
        assert_eq!(forward_axis(false, true, false, autorun), -1);
        assert_eq!(forward_axis(false, false, false, autorun), 0);
        let autorun = true;
        assert!(!autorun_cancelled(false, false, false));
        assert_eq!(forward_axis(false, true, false, autorun), 0);
        assert_eq!(forward_axis(false, false, false, autorun), 1);
    }

    #[test]
    fn both_button_run_shares_the_axis_and_replaces_autorun() {
        assert_eq!(forward_axis(false, false, true, false), 1);
        assert_eq!(forward_axis(false, true, true, false), 0);
        assert!(autorun_cancelled(false, false, true));
    }

    #[test]
    fn the_step_constants_are_the_clients() {
        assert!((STEP_UP_ADVANCE - STEP_UP_HEIGHT * 50f32.to_radians().tan()).abs() < 1e-4);
        assert!((GROUND_COS - 50f32.to_radians().cos()).abs() < 1e-6);
        assert!((STEP_SLOPE_RATIO.atan().to_degrees() - 61.6).abs() < 0.05);
        let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
        assert!((apex - 1.640).abs() < 1e-3);
    }
}
