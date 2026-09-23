#![allow(clippy::float_cmp, clippy::manual_midpoint)]

use super::*;

/// `CreatureModelData` collision heights × display scale for three real bodies.
const HUMAN_MALE: f32 = 2.031;
const GNOME_FEMALE: f32 = 1.150;
const NIGHT_ELF_MALE: f32 = 2.438;

fn player_of_height(y: f32, h: f32) -> Player {
    Player {
        pos: Vec3::new(0.0, y, 0.0),
        collision_height: h,
        ..Player::default()
    }
}

#[test]
fn leaving_the_water_by_depth_levels_the_pitch_and_a_breach_does_not() {
    let deep = swim_enter_depth(HUMAN_MALE) + 1.0;
    let mut player = player_of_height(0.0, HUMAN_MALE);
    player.mover_pitch = -0.9;
    assert!(update_swimming(&mut player, Some(deep), 0.0));
    assert_eq!(player.mover_pitch, -0.9);
    assert!(update_swimming(
        &mut player,
        Some(swim_exit_depth(HUMAN_MALE) + 0.001),
        0.1
    ));
    assert_eq!(player.mover_pitch, -0.9);
    assert!(!update_swimming(&mut player, Some(0.5), 0.2));
    assert_eq!(player.mover_pitch, 0.0);
    player.mover_pitch = -0.9;
    assert!(update_swimming(&mut player, Some(deep), 1.0));
    assert!(!update_swimming(&mut player, None, 1.1));
    assert_eq!(player.mover_pitch, 0.0);
    player.mover_pitch = -0.9;
    assert!(update_swimming(&mut player, Some(deep), 2.0));
    player.swimming = false;
    player.vel_y = 0.0;
    update_swimming(&mut player, Some(deep), 2.1);
    assert_eq!(player.mover_pitch, -0.9, "a jump out keeps its aim");
}

#[test]
fn swim_entry_and_exit_hysteresis() {
    for h in [HUMAN_MALE, GNOME_FEMALE, NIGHT_ELF_MALE] {
        assert!((swim_enter_depth(h) - swim_exit_depth(h) - 1.0 / 36.0).abs() < 1e-6);
        assert!((swim_enter_depth(h) - 0.75 * h).abs() < 1e-6);
    }
    let (enter, exit) = (swim_enter_depth(HUMAN_MALE), swim_exit_depth(HUMAN_MALE));
    let mut p = player_of_height(0.0, HUMAN_MALE);
    let mid_band = (enter + exit) * 0.5;
    assert!(!update_swimming(&mut p, Some(enter), 0.0), "strict >");
    assert!(!update_swimming(&mut p, Some(mid_band), 0.0));
    assert!(update_swimming(&mut p, Some(enter + 0.01), 0.0));
    assert!(
        update_swimming(&mut p, Some(mid_band), 0.0),
        "the band holds"
    );
    assert!(!update_swimming(&mut p, Some(exit - 0.01), 0.0));
    assert!(update_swimming(&mut p, Some(enter + 0.5), 0.0));
    assert!(!update_swimming(&mut p, None, 0.0), "no liquid, no swim");
}

#[test]
fn the_hop_relatches_at_half_launch_velocity() {
    let half_decay = SWIM_JUMP_SPEED / (2.0 * GRAVITY);
    let deep = Some(swim_enter_depth(HUMAN_MALE) + 1.0);
    let mut p = player_of_height(0.0, HUMAN_MALE);
    p.airborne_since = Some(0.0);
    p.jump_zspeed = SWIM_JUMP_SPEED;
    p.vel_y = SWIM_JUMP_SPEED;
    assert!(!update_swimming(&mut p, deep, half_decay * 0.5));
    p.vel_y = SWIM_JUMP_SPEED * 0.49;
    assert!(update_swimming(&mut p, deep, half_decay + 1e-3));
    let mut q = player_of_height(0.0, HUMAN_MALE);
    q.airborne_since = Some(0.0);
    q.jump_zspeed = SWIM_JUMP_SPEED;
    q.vel_y = -0.1;
    assert!(
        update_swimming(&mut q, deep, 0.01),
        "a fall into depth enters"
    );
}

#[test]
fn the_rest_line_is_satisfied_from_above_and_never_pulls_up() {
    for h in [HUMAN_MALE, GNOME_FEMALE, NIGHT_ELF_MALE] {
        let cap = rest_cap(h);
        assert_eq!(settle_to_rest(10.0 - cap, 10.0, h), 0.0);
        assert_eq!(settle_to_rest(10.0 - cap - 0.01, 10.0, h), 0.0);
        assert_eq!(settle_to_rest(10.0 - cap - 30.0, 10.0, h), 0.0);
        assert!((settle_to_rest(10.0 - cap + 0.25, 10.0, h) - 0.25).abs() < 1e-6);
    }
}

#[test]
fn every_race_floats_with_its_head_out_of_the_water() {
    for h in [HUMAN_MALE, GNOME_FEMALE, NIGHT_ELF_MALE] {
        let submerged = rest_cap(h);
        assert!(submerged < h);
        assert!(((h - submerged) / h - 0.25).abs() < 1e-6);
    }
}

#[test]
fn a_descending_surface_does_not_flap_the_swim_latch() {
    const SLOPE: f32 = 0.099;
    const DT: f32 = 1.0 / 60.0;
    let cap = rest_cap(HUMAN_MALE);
    let surface_after = |secs: f32| 100.0 - SLOPE * SWIM_SPEED * secs;

    let mut frozen = player_of_height(surface_after(0.0) - cap, HUMAN_MALE);
    frozen.swimming = true;
    let left_at = (0..600)
        .map(|i| i as f32 * DT)
        .find(|&t| !update_swimming(&mut frozen, Some(surface_after(t)), t))
        .expect("frozen feet cannot hold the latch");
    assert!(left_at < 0.1, "{left_at}");

    let mut held = player_of_height(surface_after(0.0) - cap, HUMAN_MALE);
    held.swimming = true;
    for i in 0..600 {
        let t = i as f32 * DT;
        let surface = surface_after(t);
        held.pos.y -= settle_to_rest(held.pos.y, surface, HUMAN_MALE);
        assert!(
            update_swimming(&mut held, Some(surface), t),
            "left at {t:.2}s"
        );
        assert!((surface - held.pos.y - cap).abs() < 1e-4);
    }
}

#[test]
fn the_rest_line_redirects_the_stroke_level_at_full_speed() {
    let free = Vec3::new(0.1, 3.0, 0.2);
    assert_eq!(cap_redirect(free, 100.0), (free, None));
    assert_eq!(cap_redirect(free, f32::INFINITY), (free, None));
    let dive = Vec3::new(1.0, -2.0, 0.0);
    assert_eq!(cap_redirect(dive, 0.0), (dive, None));
    let steep = Vec3::new(0.08, 4.72, 0.0);
    let (vel, pitch) = cap_redirect(steep, 0.0);
    assert!((vel.length() - steep.length()).abs() < 1e-4);
    assert_eq!(vel.y, 0.0);
    assert!(vel.x > 4.7);
    assert_eq!(pitch, Some(0.0));
    let (vel2, pitch2) = cap_redirect(steep, 2.0);
    assert!((vel2.length() - steep.length()).abs() < 1e-4);
    assert_eq!(vel2.y, 2.0);
    let eased = pitch2.expect("the cap bit");
    assert!(eased > 0.0 && eased < steep.y.atan2(steep.x));
}
