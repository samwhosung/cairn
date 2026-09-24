use std::f32::consts::{FRAC_PI_2, PI};

use bevy::math::ops;

use super::*;

fn moving(flags: u32, orientation: f32) -> RemoteMotion {
    let mv = RelayMove {
        server_ms: 0,
        wow_pos: [0.0; 3],
        orientation,
        flags,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    RemoteMotion::seeded(&mv, 0.0)
}

fn close(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-4)
}

#[test]
fn a_runner_moves_along_its_facing_and_a_strafe_to_its_left() {
    let run = moving(flags::FORWARD, 0.0).reckon(0.5);
    assert!(close(run.wow_pos, [3.5, 0.0, 0.0]), "{:?}", run.wow_pos);
    assert!((run.speed - RUN_SPEED).abs() < 1e-6);
    let pos = moving(flags::STRAFE_LEFT, 0.0).reckon(1.0).wow_pos;
    assert!(
        close(pos, [0.0, 7.0, 0.0]),
        "west is left of north: {pos:?}"
    );
    let diagonal = flags::FORWARD | flags::STRAFE_RIGHT;
    let pos = moving(diagonal, FRAC_PI_2).reckon(1.0).wow_pos;
    let d = 7.0 / 2f32.sqrt();
    assert!(
        close(pos, [d, d, 0.0]),
        "a diagonal keeps the run's speed: {pos:?}"
    );
}

#[test]
fn a_walk_outranks_a_backpedal_and_a_key_turn_turns_at_the_turn_rate() {
    let walking_back = moving(flags::BACKWARD | flags::WALK_MODE, 0.0);
    assert!((walking_back.reckon(1.0).speed - 2.5).abs() < 1e-5);
    assert!((moving(flags::BACKWARD, 0.0).reckon(1.0).speed - 4.5).abs() < 1e-5);
    let turn = moving(flags::TURN_LEFT, 0.0).reckon(0.5);
    assert!(close(turn.wow_pos, [0.0; 3]) && (turn.facing - PI / 2.0).abs() < 1e-5);
}

#[test]
fn a_swimmer_climbs_along_its_pitch() {
    let mut rm = moving(flags::SWIMMING | flags::FORWARD, 0.0);
    rm.pitch = FRAC_PI_2 / 3.0;
    let pos = rm.reckon(1.0).wow_pos;
    let (s, c) = ops::sin_cos(FRAC_PI_2 / 3.0);
    assert!(close(pos, [SWIM_SPEED * c, 0.0, SWIM_SPEED * s]), "{pos:?}");
}

#[test]
fn a_jump_is_seeded_from_its_launch_and_how_long_it_has_flown() {
    let launch = Jump {
        z_speed: -7.955_547,
        cos: 0.0,
        sin: 1.0,
        xy_speed: 7.0,
    };
    let arc = airborne(Some(launch), 0);
    assert!((arc.vertical_velocity - 7.955_547).abs() < 1e-5);
    assert!((arc.xy_velocity[1] - 7.0).abs() < 1e-5);
    let later = airborne(Some(launch), 500);
    assert!((later.vertical_velocity - (7.955_547 - GRAVITY * 0.5)).abs() < 1e-4);
    let grounded = airborne(None, 500);
    let still = grounded.vertical_velocity.hypot(grounded.xy_velocity[0]);
    assert!(still.hypot(grounded.xy_velocity[1]) < 1e-9);
    let mut rm = moving(flags::FALLING, 0.0);
    rm.vertical_velocity = 7.955_547;
    let peak = 7.955_547 / GRAVITY;
    let top = rm.reckon(peak);
    assert!(top.vertical_velocity.abs() < 1e-4);
    assert!((top.wow_pos[2] - 7.955_547 * peak / 2.0).abs() < 1e-4);
}

#[test]
fn a_blend_lands_on_the_waiting_move_as_its_time_comes() {
    let target = [10.0, 0.0, 0.0];
    let mut pos = [0.0; 3];
    for frame in 0..6 {
        let remaining = 0.1 - frame as f32 * 0.02;
        pos = reconcile_lerp(pos, pos, target, false, 0.02, remaining - 0.02);
    }
    assert!(close(pos, target), "{pos:?}");
    let near = [0.01, 0.0, 0.0];
    let kept = reconcile_lerp(near, near, [0.0; 3], false, 0.1, 0.1);
    assert!(close(kept, near), "inside the tolerance");
    let turned = facing_lerp(0.1, 2.0 * PI - 0.1, 0.05, 0.05);
    assert!(turned.abs() < 1e-5, "the short way round: {turned}");
}

#[test]
fn a_frame_that_ran_late_is_not_taken_for_a_late_network() {
    let at = |server_ms: u32, flags: u32, x: f32| RelayMove {
        server_ms,
        wow_pos: [x, 0.0, 0.0],
        orientation: 0.0,
        flags,
        pitch: 0.0,
        fall_time: 0,
        jump: None,
    };
    let mut rm = RemoteMotion::seeded(&at(0, 0, 0.0), 0.0);
    for i in 1..=20u16 {
        let ms = u32::from(i) * 50;
        rm.relayed(
            at(ms, flags::FORWARD, f32::from(i) * 0.35),
            f64::from(ms),
            3000.0,
        );
    }
    rm.relayed(at(1050, 0, 7.5), 1050.0, 3000.0);
    assert!(
        rm.pending.is_empty() && rm.flags == 0 && (rm.wow_pos[0] - 7.5).abs() < 1e-6,
        "on time off the socket and taken three seconds on, every move was due"
    );
    rm.relayed(at(5000, flags::FORWARD, 7.5), 5000.0, 5000.0);
    assert!(
        rm.pending.is_empty() && rm.flags == flags::FORWARD,
        "the late frame sized no buffer, so a start from rest applies as it arrives"
    );
}
