//! The move-flag word each frame carries, and the airborne arc's bookkeeping.

use super::input::MoveAxes;
use super::state::{FALL_FAR_DROP, FALL_FAR_TIME, Player};

pub const FORWARD: u32 = 0x1;
pub const BACKWARD: u32 = 0x2;
pub const STRAFE_LEFT: u32 = 0x4;
pub const STRAFE_RIGHT: u32 = 0x8;
pub const TURN_LEFT: u32 = 0x10;
pub const TURN_RIGHT: u32 = 0x20;
pub const WALK_MODE: u32 = 0x100;
pub const FALLING: u32 = 0x2000;
pub const FALLING_FAR: u32 = 0x4000;
pub const SWIMMING: u32 = 0x20_0000;
pub const ANY_MOVE: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;

pub struct FrameFlags {
    /// Direction bits stay live mid-air.
    pub live: u32,
    /// What an animation reads: direction bits frozen at take-off.
    pub pose: u32,
}

/// `swim` is `Some` exactly while swimming.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub fn this_frame(
    player: &mut Player,
    axes: &MoveAxes,
    swim: Option<(f32, f32)>,
    airborne: bool,
    jumped: bool,
    held: bool,
    air_nudged: bool,
    now: f32,
    launch_y: f32,
) -> FrameFlags {
    let mut live = 0;
    if let Some((swim_fwd, swim_side)) = swim {
        live |= SWIMMING;
        if swim_fwd < 0.0 {
            live |= BACKWARD;
        } else if swim_fwd > 0.0 {
            live |= FORWARD;
        }
        if swim_side < 0.0 {
            live |= STRAFE_LEFT;
        } else if swim_side > 0.0 {
            live |= STRAFE_RIGHT;
        }
        player.airborne_since = None;
        player.fall_far = false;
    } else {
        match axes.fwd.signum() {
            1 => live |= FORWARD,
            -1 => live |= BACKWARD,
            _ => {}
        }
        match axes.side.signum() {
            -1 => live |= STRAFE_LEFT,
            1 => live |= STRAFE_RIGHT,
            _ => {}
        }
        if !axes.mouselook {
            if axes.turn_left {
                live |= TURN_LEFT;
            }
            if axes.turn_right {
                live |= TURN_RIGHT;
            }
        }
        let new_arc = player.advance_airborne_arc(airborne, jumped, now, launch_y);
        if airborne {
            live |= FALLING;
            if new_arc || air_nudged {
                player.airborne_dirs = live & ANY_MOVE;
            }
            if player.fall_far {
                live |= FALLING_FAR;
            }
        }
        if held {
            live = 0;
        }
    }
    if player.walking {
        live |= WALK_MODE;
    }
    let pose = if airborne {
        (live & !ANY_MOVE) | player.airborne_dirs
    } else {
        live
    };
    FrameFlags { live, pose }
}

impl Player {
    /// Runs after the mover; `launch_y` is the feet height the step started from. Returns whether
    /// a new arc began.
    pub fn advance_airborne_arc(
        &mut self,
        airborne: bool,
        jumped: bool,
        now: f32,
        launch_y: f32,
    ) -> bool {
        let was_airborne = self.airborne_since.is_some();
        let new_arc = airborne && (!was_airborne || jumped);
        if new_arc {
            self.airborne_since = Some(now);
            self.jump_zspeed = if jumped { self.launch_vz } else { 0.0 };
            self.fall_start_y = launch_y;
            self.fall_far = false;
        } else if !airborne {
            self.airborne_since = None;
            self.arc_steer_spent = false;
        }
        if airborne {
            let far = if self.jump_zspeed == 0.0 {
                self.airborne_since
                    .is_some_and(|t0| now - t0 >= FALL_FAR_TIME)
            } else {
                self.pos.y <= self.fall_start_y - FALL_FAR_DROP
            };
            if far {
                self.fall_far = true;
            }
        }
        new_arc
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use bevy::prelude::Vec3;

    use super::*;
    use crate::player::state::{GRAVITY, JUMP_SPEED};

    const TAKEOFF_RISE: f32 = JUMP_SPEED / 60.0;
    const _: () = assert!(TAKEOFF_RISE > FALL_FAR_DROP);

    fn take_off(ground: f32) -> Player {
        let mut p = Player {
            vel_y: JUMP_SPEED,
            launch_vz: JUMP_SPEED,
            pos: Vec3::new(0.0, ground, 0.0),
            ..Player::default()
        };
        assert!(p.advance_airborne_arc(true, true, 0.0, ground));
        p.pos.y = ground + TAKEOFF_RISE;
        p
    }

    #[test]
    fn the_arc_snapshots_the_launch_speed_and_the_true_ground() {
        let mut p = Player {
            vel_y: JUMP_SPEED - GRAVITY / 60.0,
            launch_vz: JUMP_SPEED,
            pos: Vec3::new(0.0, 100.0 + TAKEOFF_RISE, 0.0),
            ..Player::default()
        };
        assert!(p.advance_airborne_arc(true, true, 0.0, 100.0));
        assert_eq!(p.jump_zspeed, JUMP_SPEED);
        assert_eq!(p.fall_start_y, 100.0);
        assert!(!p.fall_far);
    }

    #[test]
    fn a_flat_jump_never_latches_falling_far() {
        let mut p = take_off(100.0);
        for (t, y) in [(0.10, 100.6), (0.20, 100.75), (0.30, 100.4), (0.45, 100.0)] {
            p.pos.y = y;
            p.advance_airborne_arc(true, false, t, 100.0);
            assert!(!p.fall_far, "y={y}");
        }
    }

    #[test]
    fn a_same_frame_land_and_relaunch_is_a_new_arc() {
        let mut p = take_off(100.3);
        p.pos.y = 100.0;
        p.advance_airborne_arc(true, false, 0.4, 100.3);
        p.pos.y = 90.0;
        p.advance_airborne_arc(true, false, 0.6, 100.3);
        assert!(p.fall_far);
        assert!(p.advance_airborne_arc(true, true, 0.62, 90.0));
        assert_eq!(p.airborne_since, Some(0.62));
        assert_eq!(p.fall_start_y, 90.0);
        assert!(!p.fall_far);
    }

    #[test]
    fn a_step_off_latches_falling_far_by_time() {
        let mut p = Player {
            pos: Vec3::new(0.0, 100.0 - GRAVITY / 3600.0, 0.0),
            vel_y: -0.5,
            ..Player::default()
        };
        assert!(p.advance_airborne_arc(true, false, 0.0, 100.0));
        assert_eq!(p.jump_zspeed, 0.0);
        p.pos.y = 99.95;
        p.advance_airborne_arc(true, false, 0.3, 100.0);
        assert!(!p.fall_far);
        p.advance_airborne_arc(true, false, FALL_FAR_TIME + 0.01, 100.0);
        assert!(p.fall_far);
    }

    #[test]
    fn a_clean_landing_clears_the_clock_and_the_steer() {
        let mut p = take_off(100.0);
        p.arc_steer_spent = true;
        p.pos.y = 100.0;
        assert!(!p.advance_airborne_arc(false, false, 0.5, 100.0));
        assert_eq!(p.airborne_since, None);
        assert!(!p.arc_steer_spent);
    }

    #[test]
    fn the_walk_gait_outlives_the_settle() {
        let axes = MoveAxes {
            fwd: 1,
            ..MoveAxes::default()
        };
        let mut player = Player {
            walking: true,
            ..Player::default()
        };
        let f = this_frame(
            &mut player,
            &axes,
            None,
            false,
            false,
            true,
            false,
            1.0,
            0.0,
        );
        assert_eq!(f.live, WALK_MODE, "the settle clears motion, not the mode");
        let mut runner = Player::default();
        let f = this_frame(
            &mut runner,
            &axes,
            None,
            false,
            false,
            false,
            false,
            1.0,
            0.0,
        );
        assert_eq!(f.live, FORWARD);
    }
}
