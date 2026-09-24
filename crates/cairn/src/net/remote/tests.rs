use std::f32::consts::{FRAC_PI_2, PI};

use bevy::math::ops;

use super::*;

fn moving(flags: u32, orientation: f32) -> RemoteMotion {
    let mv = RelayMove {
        wire_ms: 0,
        position: [0.0; 3],
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
    let (pos, _, _, speed) = moving(flags::FORWARD, 0.0).advance(0.5);
    assert!(
        close(pos, [3.5, 0.0, 0.0]) && (speed - RUN_SPEED).abs() < 1e-6,
        "{pos:?}"
    );
    let (pos, ..) = moving(flags::STRAFE_LEFT, 0.0).advance(1.0);
    assert!(
        close(pos, [0.0, 7.0, 0.0]),
        "west is left of north: {pos:?}"
    );
    let (pos, ..) = moving(flags::FORWARD | flags::STRAFE_RIGHT, FRAC_PI_2).advance(1.0);
    let d = 7.0 / 2f32.sqrt();
    assert!(
        close(pos, [d, d, 0.0]),
        "a diagonal keeps the run's speed: {pos:?}"
    );
}

#[test]
fn a_walk_outranks_a_backpedal_and_a_key_turn_turns_at_the_turn_rate() {
    let walking_back = moving(flags::BACKWARD | flags::WALK_MODE, 0.0);
    assert!((walking_back.advance(1.0).3 - 2.5).abs() < 1e-5);
    assert!((moving(flags::BACKWARD, 0.0).advance(1.0).3 - 4.5).abs() < 1e-5);
    let (pos, facing, ..) = moving(flags::TURN_LEFT, 0.0).advance(0.5);
    assert!(close(pos, [0.0; 3]) && (facing - PI / 2.0).abs() < 1e-5);
}

#[test]
fn a_swimmer_climbs_along_its_pitch() {
    let mut rm = moving(flags::SWIMMING | flags::FORWARD, 0.0);
    rm.pitch = FRAC_PI_2 / 3.0;
    let (pos, ..) = rm.advance(1.0);
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
    let (vz, xy) = jump_seed(Some(launch), 0);
    assert!((vz - 7.955_547).abs() < 1e-5 && (xy[1] - 7.0).abs() < 1e-5);
    let (vz, _) = jump_seed(Some(launch), 500);
    assert!((vz - (7.955_547 - GRAVITY * 0.5)).abs() < 1e-4);
    assert_eq!(jump_seed(None, 500), (0.0, [0.0; 2]));
    let mut rm = moving(flags::FALLING, 0.0);
    rm.vertical_velocity = 7.955_547;
    let peak = 7.955_547 / GRAVITY;
    let (pos, _, vz, _) = rm.advance(peak);
    assert!(vz.abs() < 1e-4 && (pos[2] - 7.955_547 * peak / 2.0).abs() < 1e-4);
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
    assert!(
        close(reconcile_lerp(near, near, [0.0; 3], false, 0.1, 0.1), near),
        "inside the tolerance"
    );
    let turned = facing_lerp(0.1, 2.0 * PI - 0.1, 0.05, 0.05);
    assert!((turned - 0.0).abs() < 1e-5, "the short way round: {turned}");
}
