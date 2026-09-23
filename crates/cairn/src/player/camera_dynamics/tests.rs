#![allow(clippy::float_cmp)]

use super::*;
use crate::player::camera_channel::assert_bounded_step;

const DT: f32 = 1.0 / 60.0;
const UP: f32 = 0.01;

fn moving(translating: bool) -> SubjectState {
    SubjectState {
        last_move_flags: if translating { FORWARD } else { 0 },
        ..SubjectState::default()
    }
}

#[test]
fn only_a_clipped_camera_looking_level_or_up_and_not_translating_pivots() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    assert_eq!(
        p.route_pitch(UP, 0.0, 0.2, &moving(false), true, &cfg),
        None
    );
    assert!(p.bias() > 0.0);
    let off = CameraOptions {
        pivot: false,
        ..cfg
    };
    for (name, opts, pitch, subject, clipped) in [
        ("unclipped", cfg, 0.2, moving(false), false),
        ("translating", cfg, 0.2, moving(true), true),
        ("pitched down", cfg, -0.2, moving(false), true),
        ("option off", off, 0.2, moving(false), true),
    ] {
        let mut p = SmartPivot::default();
        assert_eq!(
            p.route_pitch(UP, 0.0, pitch, &subject, clipped, &opts),
            Some(UP),
            "{name}"
        );
        assert_eq!(p.bias(), 0.0, "{name}");
    }
}

#[test]
fn a_mostly_horizontal_drag_is_never_a_pivot() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    assert_eq!(
        p.route_pitch(
            UP,
            cfg.pivot_dx_max + 0.001,
            0.2,
            &moving(false),
            true,
            &cfg
        ),
        Some(UP)
    );
    assert_eq!(
        p.route_pitch(UP, cfg.pivot_dx_max, 0.2, &moving(false), true, &cfg),
        Some(UP)
    );
    assert_eq!(p.bias(), 0.0);
    assert_eq!(
        p.route_pitch(
            UP,
            cfg.pivot_dx_max - 0.001,
            0.2,
            &moving(false),
            true,
            &cfg
        ),
        None
    );
}

#[test]
fn the_bias_cannot_take_the_composite_past_the_pitch_limit() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    for _ in 0..400 {
        p.route_pitch(UP, 0.0, 0.5, &moving(false), true, &cfg);
    }
    assert!((p.bias() - (CAM_PITCH_LIMIT - 0.5)).abs() < 1e-5);
}

#[test]
fn dragging_back_down_returns_the_axis_to_the_ordinary_pitch() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    for _ in 0..5 {
        assert_eq!(
            p.route_pitch(UP, 0.0, 0.2, &moving(false), true, &cfg),
            None
        );
    }
    let peak = p.bias();
    let handed_back = (0..6)
        .filter(|_| {
            p.route_pitch(-UP, 0.0, 0.2, &moving(false), true, &cfg)
                .is_some()
        })
        .count();
    assert!(handed_back > 0 && p.bias() < peak);
}

#[test]
fn losing_the_gate_eases_the_bias_home_at_the_target_smooth_speed() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    p.route_pitch(0.3, 0.0, 0.5, &moving(false), true, &cfg);
    let held = p.bias();
    for _ in 0..120 {
        p.advance(0.5, &moving(false), true, &cfg, DT);
    }
    assert_eq!(p.bias(), held);
    let expected = held / cfg.target_smooth_speed.to_radians();
    let took = (0..600)
        .find(|_| {
            p.advance(0.5, &moving(false), false, &cfg, DT);
            p.bias().abs() < CHANNEL_EPS
        })
        .map(|f| (f + 1) as f32 * DT)
        .expect("the bias comes home");
    assert!((took - expected).abs() < 0.05, "{took} vs {expected}");
}

#[test]
fn re_entering_the_gate_cancels_the_return_where_it_stands() {
    let cfg = CameraOptions::default();
    let mut p = SmartPivot::default();
    p.route_pitch(0.3, 0.0, 0.5, &moving(false), true, &cfg);
    for _ in 0..6 {
        p.advance(0.5, &moving(false), false, &cfg, DT);
    }
    let mid = p.bias();
    assert!(mid > CHANNEL_EPS && mid < 0.3);
    for _ in 0..60 {
        p.advance(0.5, &moving(false), true, &cfg, DT);
    }
    assert_eq!(p.bias(), mid);
}

#[test]
fn the_terrain_staircase_is_five_signed_steps() {
    let deg = |slope: f32| TerrainTilt::slope_to_pitch(slope).to_degrees();
    for (slope, want) in [
        (0.0, 0.0),
        (0.089, 0.0),
        (0.09, 5.0),
        (0.18, 10.0),
        (0.27, 15.0),
        (0.35, 15.0),
        (0.36, 20.0),
        (100.0, 20.0),
    ] {
        assert!((deg(slope) - want).abs() < 1e-4, "{slope}");
        assert!((deg(-slope) + want).abs() < 1e-4, "-{slope}");
    }
}

#[test]
fn the_tilt_matrix_disables_smart_idle_and_levels_swim() {
    for state in [
        TiltState::Fall,
        TiltState::Idle,
        TiltState::Move,
        TiltState::Swim,
    ] {
        assert!(state.row(FollowStyle::Never).factor < 0.0);
    }
    assert!(TiltState::Idle.row(FollowStyle::Smart).factor < 0.0);
    assert!(TiltState::Idle.row(FollowStyle::Always).factor > 0.0);
    assert_eq!(TiltState::Swim.row(FollowStyle::Smart).absorb, 0.0);
    assert_eq!(TiltState::Fall.row(FollowStyle::Smart).factor, 0.75);
    let of = |f| {
        TiltState::of(&SubjectState {
            last_move_flags: f,
            ..SubjectState::default()
        })
    };
    assert_eq!(of(SWIMMING | FALLING | FORWARD), TiltState::Swim);
    assert_eq!(of(FALLING | FORWARD), TiltState::Fall);
    assert_eq!(of(TURN_RIGHT), TiltState::Turn);
}

#[test]
fn the_tilt_channel_is_throttled_gated_and_floored_at_three_seconds() {
    let cfg = CameraOptions {
        terrain_tilt: true,
        ..CameraOptions::default()
    };
    let subject = moving(true);
    let mut t = TerrainTilt::default();
    let mut probes = 0;
    for _ in 0..6 {
        t.advance(
            || {
                probes += 1;
                0.5
            },
            true,
            &subject,
            FollowStyle::Smart,
            &cfg,
            DT,
        );
    }
    assert_eq!(probes, 1);
    let mut t = TerrainTilt::default();
    let target = TerrainTilt::slope_to_pitch(0.5);
    let took = (0..600)
        .find(|_| t.advance(|| 0.5, true, &subject, FollowStyle::Smart, &cfg, DT) == target)
        .map(|f| f as f32 * DT)
        .expect("the lean arrives");
    assert!((took - cfg.tilt_time_min).abs() < 0.05, "{took}");
    let mut zeroed = 0.0_f32;
    for _ in 0..600 {
        zeroed = t.advance(|| 0.5, false, &subject, FollowStyle::Smart, &cfg, DT);
    }
    assert!(zeroed.abs() < CHANNEL_EPS);
}

#[test]
fn mouse_look_hands_the_lean_into_the_pitch_and_takes_it_back() {
    let cfg = CameraOptions {
        terrain_tilt: true,
        ..CameraOptions::default()
    };
    let subject = moving(true);
    let mut tilt = TerrainTilt::default();
    for _ in 0..900 {
        tilt.advance(|| 0.5, true, &subject, FollowStyle::Smart, &cfg, DT);
    }
    let lean = tilt.pitch();
    assert!(lean > 0.0);
    let mut pitch = 0.2_f32;
    let composite = pitch + tilt.pitch();
    pitch += tilt.hand_off(true);
    assert_eq!(tilt.pitch(), 0.0);
    assert!((pitch - composite).abs() < 1.0e-6);
    assert_eq!(tilt.hand_off(true), 0.0);
    pitch += tilt.hand_off(false);
    assert!((pitch - 0.2).abs() < 1.0e-6);
    pitch += tilt.hand_off(true);
    for _ in 0..900 {
        tilt.advance(|| 0.0, true, &subject, FollowStyle::Smart, &cfg, DT);
    }
    pitch += tilt.hand_off(false);
    assert!((pitch - (0.2 + lean)).abs() < 1.0e-6);
}

#[test]
fn head_bob_arms_on_the_movement_command_word_and_the_mouse_chord() {
    use follow_cmd as c;
    for bits in [
        c::FORWARD,
        c::BACKWARD,
        c::STRAFE_LEFT,
        c::AUTORUN,
        c::RIGHT_MOUSE | c::LEFT_MOUSE,
        c::RIGHT_MOUSE | c::TURN_LEFT,
    ] {
        assert!(HeadBob::arms(bits), "{bits:#x}");
    }
    for bits in [
        0,
        c::RIGHT_MOUSE,
        c::LEFT_MOUSE,
        c::TURN_LEFT,
        c::LEFT_MOUSE | c::TURN_LEFT,
    ] {
        assert!(!HeadBob::arms(bits), "{bits:#x}");
    }
}

#[test]
fn head_bob_is_first_person_only_and_every_conjunct_can_stop_it() {
    let cfg = CameraOptions {
        bobbing: true,
        ..CameraOptions::default()
    };
    let running = SubjectState {
        camera_command: follow_cmd::FORWARD,
        last_move_flags: FORWARD,
        speed: 7.0,
        ..SubjectState::default()
    };
    let bobs = |zoom: f32, subject: &SubjectState, cfg: &CameraOptions| {
        let mut b = HeadBob::default();
        for _ in 0..31 {
            b.advance(zoom, subject, cfg, DT);
        }
        b.offset() != Vec3::ZERO
    };
    assert!(bobs(0.0, &running, &cfg));
    assert!(bobs(BOB_FIRST_PERSON_DISTANCE, &running, &cfg));
    assert!(!bobs(BOB_FIRST_PERSON_DISTANCE + 0.001, &running, &cfg));
    let off = CameraOptions {
        bobbing: false,
        ..cfg
    };
    assert!(!bobs(0.0, &running, &off));
    for flags in [FORWARD | SWIMMING, FORWARD | FALLING] {
        let s = SubjectState {
            last_move_flags: flags,
            ..running
        };
        assert!(!bobs(0.0, &s, &cfg), "{flags:#x}");
    }
    let still = SubjectState {
        camera_command: 0,
        ..running
    };
    assert!(!bobs(0.0, &still, &cfg));
}

#[test]
fn releasing_the_key_ramps_the_bob_to_an_exact_zero() {
    let cfg = CameraOptions {
        bobbing: true,
        ..CameraOptions::default()
    };
    let running = SubjectState {
        camera_command: follow_cmd::FORWARD,
        last_move_flags: FORWARD,
        speed: BOB_SPEED_DIVISOR,
        ..SubjectState::default()
    };
    let dt = 1.0 / 600.0;
    let mut b = HeadBob::default();
    for _ in 0..151 {
        b.advance(0.0, &running, &cfg, dt);
    }
    let expected = b.offset().abs().max_element() / cfg.bob_smooth_speed;
    let idle = SubjectState {
        camera_command: 0,
        ..running
    };
    let took = (0..2000)
        .find(|_| {
            b.advance(0.0, &idle, &cfg, dt);
            b.offset() == Vec3::ZERO
        })
        .map(|f| f as f32 * dt)
        .expect("the residual comes home");
    assert!((took - expected).abs() < 0.02, "{took} vs {expected}");
}

#[test]
fn the_staircases_jumps_reach_the_camera_as_a_smooth_lean() {
    assert_bounded_step(
        (-0.6, 0.6),
        0.0005,
        5.0_f32.to_radians() + 1.0e-6,
        TerrainTilt::slope_to_pitch,
    );
    let cfg = CameraOptions {
        terrain_tilt: true,
        ..CameraOptions::default()
    };
    let mut tilt = TerrainTilt::default();
    assert_bounded_step((0.0, 20.0), DT, 0.25_f32.to_radians(), |now| {
        let slope = -0.5 + now * 0.05;
        tilt.advance(|| slope, true, &moving(true), FollowStyle::Smart, &cfg, DT)
    });
}
