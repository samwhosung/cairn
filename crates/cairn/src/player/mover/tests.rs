#![allow(clippy::float_cmp)]

use super::*;

/// Outward normal of a face rising toward +x, tilted `deg` from horizontal.
fn face(deg: f32) -> Vec3 {
    let r = deg.to_radians();
    Vec3::new(-r.sin(), r.cos(), 0.0)
}

#[test]
fn a_walkable_ramp_rides_at_full_horizontal_speed() {
    let n = face(45.0);
    let ride = walkable_ride_velocity(n, Vec3::new(7.0, 0.0, 0.0)).expect("rides");
    assert_eq!((ride.x, ride.z), (7.0, 0.0));
    assert!(ride.y > 0.0 && ride.dot(n).abs() < 1e-6);
}

#[test]
fn a_diagonal_approach_is_not_deflected() {
    let ride = walkable_ride_velocity(face(40.0), Vec3::new(5.0, 0.0, 5.0)).expect("rides");
    assert_eq!((ride.x, ride.z), (5.0, 5.0));
}

#[test]
fn a_prior_facet_ride_is_recomputed_not_stacked() {
    let n = face(45.0);
    let ride = walkable_ride_velocity(n, Vec3::new(7.0, 3.0, 0.0)).expect("rides");
    assert_eq!((ride.x, ride.z), (7.0, 0.0));
    assert!(ride.dot(n).abs() < 1e-6);
}

#[test]
fn steep_flat_and_receding_planes_never_ride() {
    let push = Vec3::new(7.0, 0.0, 0.0);
    assert!(walkable_ride_velocity(face(60.0), push).is_none());
    assert!(walkable_ride_velocity(Vec3::Y, push).is_none());
    assert!(walkable_ride_velocity(face(40.0), -push).is_none());
}

#[test]
fn the_ride_covers_the_walkable_range_up_to_the_gate() {
    let v = Vec3::new(7.0, 0.0, 0.0);
    let ride = walkable_ride_velocity(face(49.9), v).expect("rides");
    assert_eq!((ride.x, ride.z), (7.0, 0.0));
    assert!(ride.y <= 7.0 * 50.0_f32.to_radians().tan() + 1e-3);
    assert!(walkable_ride_velocity(face(50.1), v).is_none());
    assert!(steep_contact_shear(face(50.1), v).is_some());
}

#[test]
fn walking_into_a_steep_face_spends_the_push_along_it() {
    let out = steep_contact_shear(face(60.0), Vec3::new(7.0, 0.0, 0.0)).expect("strips");
    assert!(out.x.abs() < 1e-6 && out.y == 0.0);
}

#[test]
fn a_fall_against_a_face_keeps_the_whole_of_its_descent() {
    let (n, v) = (face(55.0), Vec3::new(7.0, -4.9, 0.0));
    let orthogonal = v - v.dot(n) * n;
    assert!(orthogonal.y > -0.2, "the orthogonal clip nearly cancels");
    let out = steep_contact_shear(n, v).expect("responds");
    assert!((out.y - v.y).abs() < 1.0e-4);
    let cot = 1.0 / 55.0_f32.to_radians().tan();
    assert!((out.x - v.y * cot).abs() < 1.0e-3);
    assert!(out.dot(n).abs() < 1.0e-4);
}

#[test]
fn a_plumb_fall_is_carried_down_the_face_at_full_speed() {
    let out = steep_contact_shear(face(60.0), Vec3::new(0.0, -10.0, 0.0)).expect("follows");
    assert!((out.y + 10.0).abs() < 1.0e-4);
    let cot = 1.0 / 60.0_f32.to_radians().tan();
    assert!((out.x + 10.0 * cot).abs() < 1.0e-3);
    assert!(steep_contact_shear(face(60.0), Vec3::new(7.0, 20.0, 0.0)).is_none());
}

#[test]
fn a_rising_jump_keeps_its_own_lift() {
    let v = Vec3::new(7.0, 8.0, 0.0);
    let out = steep_contact_shear(face(60.0), v).expect("strips");
    assert!((out.y - v.y).abs() < 1e-6);
    assert!(out.dot(face(60.0)) >= -1e-6);
}

#[test]
fn walkable_and_overhanging_faces_are_untouched() {
    let push = Vec3::new(7.0, 0.0, 0.0);
    assert!(steep_contact_shear(face(40.0), push).is_none());
    assert!(steep_contact_shear(Vec3::new(-0.5, -0.7, 0.0).normalize(), push).is_none());
    assert!(steep_contact_shear(face(60.0), -push).is_none());
}

#[test]
fn a_vertical_wall_takes_the_whole_push_and_no_more() {
    let v = Vec3::new(7.0, -4.0, 0.0);
    let out = steep_contact_shear(face(90.0), v).expect("strips");
    assert!(out.x.abs() < 1e-6 && (out.y - v.y).abs() < 1e-6);
}

#[test]
fn the_cone_ride_gains_the_cones_slope() {
    let head_on = foot_cone_ride(Vec3::new(-1.0, 0.0, 0.0), Vec3::X * 7.0).expect("rides");
    assert!((head_on.y - 7.0 * STEP_SLOPE_RATIO).abs() < 1e-4);
    assert_eq!(head_on.x, 7.0);
    let oblique = foot_cone_ride(
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(7.0 * 0.5, 0.0, 7.0 * 0.866_025_4),
    )
    .expect("rides");
    assert!((oblique.y - 7.0 * 0.5 * STEP_SLOPE_RATIO).abs() < 1e-3);
}

#[test]
fn the_cone_ride_declines_what_is_not_its_business() {
    let v = Vec3::X * 7.0;
    assert!(foot_cone_ride(Vec3::new(-0.5, 0.866, 0.0).normalize(), v).is_none());
    assert!(foot_cone_ride(Vec3::new(-0.5, -0.866, 0.0).normalize(), v).is_none());
    assert!(foot_cone_ride(Vec3::new(1.0, 0.0, 0.0), v).is_none());
}

#[test]
fn the_fall_step_is_frame_rate_independent() {
    let total = 0.5_f32;
    let drop = |steps: u32| {
        let dt = total / steps as f32;
        let (mut v, mut y) = (JUMP_SPEED, 0.0_f32);
        for _ in 0..steps {
            let (end, mean) = fall_step(v, dt, TERMINAL_VELOCITY);
            y += mean * dt;
            v = end;
        }
        y
    };
    let analytic = JUMP_SPEED * total - 0.5 * GRAVITY * total * total;
    for steps in [1_u32, 2, 8, 30, 120] {
        assert!((drop(steps) - analytic).abs() < 1.0e-4, "{steps} steps");
    }
}

#[test]
fn the_jump_apex_is_the_analytic_one_at_any_frame_rate() {
    let apex = JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY);
    for fps in [30.0_f32, 60.0, 144.0] {
        let dt = 1.0 / fps;
        let (mut v, mut y, mut high) = (JUMP_SPEED, 0.0_f32, 0.0_f32);
        while v > -1.0 {
            let (end, mean) = fall_step(v, dt, TERMINAL_VELOCITY);
            y += mean * dt;
            high = high.max(y);
            v = end;
        }
        assert!(
            (high - apex).abs() < 0.5 * GRAVITY * dt * dt + 1.0e-4,
            "{fps} fps: {high}"
        );
    }
}

#[test]
fn the_terminal_clamp_is_exact_on_the_step_that_reaches_it() {
    let dt = 1.0 / 60.0;
    let (end, mean) = fall_step(0.0, dt, TERMINAL_VELOCITY);
    assert!((end + GRAVITY * dt).abs() < 1.0e-6);
    assert!((mean + 0.5 * GRAVITY * dt).abs() < 1.0e-6);
    let v0 = -TERMINAL_VELOCITY + 0.5 * GRAVITY * dt;
    let (end, mean) = fall_step(v0, dt, TERMINAL_VELOCITY);
    assert_eq!(end, -TERMINAL_VELOCITY);
    let t_c = (v0 + TERMINAL_VELOCITY) / GRAVITY;
    let want = (0.5 * (v0 - TERMINAL_VELOCITY) * t_c - TERMINAL_VELOCITY * (dt - t_c)) / dt;
    assert!((mean - want).abs() < 1.0e-6);
    assert_eq!(
        fall_step(-TERMINAL_VELOCITY, dt, TERMINAL_VELOCITY),
        (-TERMINAL_VELOCITY, -TERMINAL_VELOCITY)
    );
}
