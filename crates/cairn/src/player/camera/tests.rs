#![allow(clippy::float_cmp)]

use super::*;

#[test]
fn the_look_rates_are_the_shipped_rate_scaled_per_axis() {
    let d = LookConfig::default();
    assert_eq!(d.yaw_rate(), LOOK_SENSITIVITY);
    assert_eq!(d.pitch_rate(), LOOK_SENSITIVITY);
    let fast = LookConfig {
        sensitivity: 1.5,
        ..LookConfig::default()
    };
    assert_eq!(fast.yaw_rate(), LOOK_SENSITIVITY * 1.5);
    let yaw = LookConfig {
        yaw_speed: 360.0,
        ..LookConfig::default()
    };
    assert_eq!(yaw.yaw_rate(), LOOK_SENSITIVITY * 2.0);
    assert_eq!(yaw.pitch_rate(), LOOK_SENSITIVITY, "one axis each");
}

#[test]
fn the_world_holds_a_button_from_its_press_to_its_release() {
    let mut mouse = WorldMouse::default();
    let mut buttons = ButtonInput::<MouseButton>::default();
    buttons.press(MouseButton::Right);
    mouse.update(&buttons, false);
    assert!(
        !mouse.held(LookButton::Right),
        "a press elsewhere is not claimed"
    );
    buttons.clear();
    mouse.update(&buttons, true);
    assert!(!mouse.held(LookButton::Right), "nor handed back mid-hold");
    buttons.release(MouseButton::Right);
    buttons.clear();

    buttons.press(MouseButton::Right);
    mouse.update(&buttons, true);
    assert!(mouse.down(LookButton::Right));
    buttons.clear();
    mouse.update(&buttons, false);
    assert!(mouse.held(LookButton::Right) && !mouse.down(LookButton::Right));
    buttons.press(MouseButton::Left);
    mouse.update(&buttons, true);
    assert!(mouse.both());
    mouse.update(&ButtonInput::default(), false);
    assert!(!mouse.held(LookButton::Right) && !mouse.held(LookButton::Left));
}

#[test]
fn the_zoom_glides_at_a_constant_speed_and_stops_on_its_target() {
    let mut rig = CameraControl::default();
    apply_zoom_scroll(5.0, 0.0, &mut rig);
    assert_eq!(rig.target_distance, CAM_DIST_DEFAULT - 5.0);
    let dt = 1.0 / 60.0;
    let mut frames = 0;
    while rig.distance > rig.target_distance {
        apply_zoom_scroll(0.0, dt, &mut rig);
        frames += 1;
    }
    assert_eq!(rig.distance, rig.target_distance);
    assert!((frames as f32 * dt - 5.0 / CAM_MOVE_SPEED).abs() < 2.0 * dt);
    apply_zoom_scroll(100.0, dt, &mut rig);
    assert_eq!(
        rig.target_distance, CAM_DIST_MIN,
        "first person is reachable"
    );
    apply_zoom_scroll(-100.0, dt, &mut rig);
    assert_eq!(rig.target_distance, CAM_DIST_MAX);
}

#[test]
fn a_right_drag_turns_the_body_and_a_left_drag_only_the_camera() {
    let dynamics = DynamicsInput {
        options: super::super::camera_dynamics::CameraOptions::default(),
        smooth_style: FollowStyle::Smart,
        subject: super::super::camera_dynamics::SubjectState::default(),
        nearclip: 0.1,
        surface_y: None,
    };
    for (button, turns_body) in [(MouseButton::Right, true), (MouseButton::Left, false)] {
        let mut rig = CameraControl::default();
        let mut buttons = ButtonInput::<MouseButton>::default();
        buttons.press(button);
        rig.world_mouse.update(&buttons, true);
        let mut cam = CameraRig::default();
        let mut face = 0.0;
        let motion = Vec2::new(-100.0, 0.0);
        run_look_session(
            &buttons, motion, false, &mut rig, &mut cam, &mut face, None, &dynamics,
        );
        assert!((cam.yaw - 100.0 * LOOK_SENSITIVITY).abs() < 1e-6);
        assert_eq!(face == cam.yaw, turns_body, "{button:?}");
        assert_eq!(rig.look_turns_body, turns_body);
    }
}
