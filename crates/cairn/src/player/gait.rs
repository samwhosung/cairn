//! The rendered body heading.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use super::flags::{BACKWARD, FORWARD, STRAFE_LEFT, STRAFE_RIGHT, TURN_LEFT, TURN_RIGHT};
use super::state::{Player, STATIONARY_CHASE_RATE};

/// The strafe ease's rate, 1/s: a quarter of the gap per frame at 60 fps.
const STRAFE_BLEND_RATE: f32 = 17.26;

/// An angle wrapped into `(−π, π]`.
pub fn wrap_pi(angle: f32) -> f32 {
    PI - (PI - angle).rem_euclid(TAU)
}

/// How far the body sits from the aim while strafing; left is positive.
pub fn strafe_body_offset(flags: u32) -> f32 {
    let left = flags & STRAFE_LEFT != 0;
    let right = flags & STRAFE_RIGHT != 0;
    if left == right {
        return 0.0;
    }
    let magnitude = if flags & (FORWARD | BACKWARD) != 0 {
        FRAC_PI_4
    } else {
        FRAC_PI_2
    };
    if left == (flags & BACKWARD == 0) {
        magnitude
    } else {
        -magnitude
    }
}

/// Eases the body's offset from the aim toward `offset`: in offset space a left-right flip swings
/// through the aim instead of tying at 180°.
pub fn ease_strafe_yaw(current_yaw: f32, aim: f32, offset: f32, dt: f32) -> f32 {
    let cur = wrap_pi(current_yaw - aim);
    let eased = cur + (offset - cur) * (1.0 - (-STRAFE_BLEND_RATE * dt).exp());
    wrap_pi(aim + eased)
}

/// Advances the body heading one frame and returns the flags an animation would read: turning in
/// place follows the body's own steps, not the keys.
#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub fn drive_body_heading(
    player: &mut Player,
    flags: u32,
    dt: f32,
    swimming: bool,
    moving: bool,
    airborne: bool,
    steering: bool,
    turn_rate: f32,
) -> u32 {
    let offset = if swimming {
        0.0
    } else {
        strafe_body_offset(flags)
    };
    let mut body_turn_step = 0.0_f32;
    if swimming || (offset == 0.0 && (moving || airborne)) {
        player.model_yaw = player.face_yaw;
    } else if offset != 0.0 {
        player.model_yaw = ease_strafe_yaw(player.model_yaw, player.face_yaw, offset, dt);
    } else {
        let delta = wrap_pi(player.face_yaw - player.model_yaw);
        let mut step = (delta.abs() - FRAC_PI_2).max(0.0);
        if !steering {
            step += dt * turn_rate * STATIONARY_CHASE_RATE;
        }
        body_turn_step = step.min(delta.abs()).copysign(delta);
        player.model_yaw = wrap_pi(player.model_yaw + body_turn_step);
    }
    let mut anim = flags;
    if !swimming && !moving && !airborne {
        anim &= !(TURN_LEFT | TURN_RIGHT);
        if body_turn_step > 1e-5 {
            anim |= TURN_LEFT;
        } else if body_turn_step < -1e-5 {
            anim |= TURN_RIGHT;
        }
    }
    anim
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn the_strafe_offset_mirrors_while_backpedalling() {
        assert_eq!(strafe_body_offset(STRAFE_LEFT), FRAC_PI_2);
        assert_eq!(strafe_body_offset(STRAFE_RIGHT), -FRAC_PI_2);
        assert_eq!(strafe_body_offset(STRAFE_LEFT | FORWARD), FRAC_PI_4);
        assert_eq!(strafe_body_offset(STRAFE_LEFT | BACKWARD), -FRAC_PI_4);
        assert_eq!(strafe_body_offset(STRAFE_LEFT | STRAFE_RIGHT), 0.0);
    }

    #[test]
    fn a_strafe_flip_swings_through_the_aim() {
        let mut yaw = FRAC_PI_2;
        let mut crossed = false;
        for _ in 0..60 {
            yaw = ease_strafe_yaw(yaw, 0.0, -FRAC_PI_2, 1.0 / 60.0);
            crossed |= yaw.abs() < 0.2;
            assert!(yaw.abs() <= FRAC_PI_2 + 1e-4, "swung round the back: {yaw}");
        }
        assert!(crossed && (yaw + FRAC_PI_2).abs() < 0.01);
    }

    #[test]
    fn the_standing_body_sweeps_back_at_the_movers_own_rate() {
        let sweep = |turn_rate: f32| {
            let mut player = Player {
                face_yaw: 1.0,
                ..Player::default()
            };
            drive_body_heading(&mut player, 0, 0.01, false, false, false, false, turn_rate);
            player.model_yaw
        };
        let (slow, fast) = (sweep(PI / 4.0), sweep(PI));
        assert!(slow > 0.0 && (fast / slow - 4.0).abs() < 1e-3);
        let mut player = Player {
            face_yaw: 1.0,
            ..Player::default()
        };
        drive_body_heading(&mut player, 0, 0.01, false, false, false, true, PI);
        assert_eq!(
            player.model_yaw, 0.0,
            "steering freezes the chase under 90°"
        );
    }
}
