//! The mover's own loop over profiled geometry.

use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::player::fixture::{frame_time, player_capsule, world_from_profile};
use crate::player::state::{STEP_UP_ADVANCE, WEDGE_MIN_FALL};

const TRAVEL_60FPS: f32 = 7.0 / 60.0;
/// A 0.28 yd kerb whose riser is a 61° bevel.
const KERB: [(f32, f32); 4] = [(-2.0, 0.0), (0.29, 0.0), (0.446, 0.28), (3.0, 0.28)];

/// `(feet height, climb, steep support, walkable ground)` per frame.
type Row = (f32, Option<f32>, bool, bool);

fn walk_from(mut app: App, start: Vec3, dir: Vec3, frames: usize) -> Vec<Row> {
    app.world_mut()
        .run_system_once(move |world: WorldCollision<'_, '_>| {
            let capsule = player_capsule();
            let cast = |from: Vec3, disp: Vec3| world.cast_body(&capsule, from, disp, SKIN_WIDTH);
            let start = start + Vec3::Y * (CAPSULE_HEIGHT * 0.5);
            let run = cast(start, dir).map_or(1.0, |h| h.distance);
            let mut center = start + dir * run;
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let mut support = Support::default();
            (0..frames)
                .map(|_| {
                    let g = grounded_step(&world, &capsule, center, dir * 7.0, dt, support);
                    center = g.center;
                    support.steep = g.steep_support;
                    (
                        center.y - CAPSULE_HEIGHT * 0.5,
                        g.climb,
                        g.steep_support,
                        g.ground.is_some(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .expect("the system runs")
}

fn walk_profile(profile: &[(f32, f32)], start_y: f32, frames: usize) -> Vec<Row> {
    walk_from(
        world_from_profile(profile),
        Vec3::new(-1.0, start_y, 0.0),
        Vec3::X,
        frames,
    )
}

fn step_at(advance: f32) -> StepVerdict {
    world_from_profile(&KERB)
        .world_mut()
        .run_system_once(move |world: WorldCollision<'_, '_>| {
            let capsule = player_capsule();
            let cast = |from: Vec3, disp: Vec3| world.cast_body(&capsule, from, disp, SKIN_WIDTH);
            let start = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5, 0.0);
            let run = cast(start, Vec3::X).map_or(1.0, |h| h.distance);
            step_up(
                &cast,
                start + Vec3::X * run,
                Vec3::X,
                TRAVEL_60FPS,
                advance,
                STEP_UP_HEIGHT,
            )
            .verdict
        })
        .expect("the system runs")
}

fn climbs_monotonically(frames: &[Row]) {
    for w in frames.windows(2) {
        assert!(w[1].0 >= w[0].0 - 1.0e-3, "went backwards: {frames:?}");
    }
}

#[test]
fn a_vertical_step_is_climbed_not_stalled() {
    const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.30, 0.0), (0.302, 0.40), (3.0, 1.48)];
    let frames = walk_profile(&P, 0.0, 6);
    assert!(frames.last().expect("frames").0 > 0.40, "{frames:?}");
    climbs_monotonically(&frames);
    assert!(
        frames[0].0 < 0.40,
        "a ride, not a one-frame pop: {frames:?}"
    );
}

#[test]
fn a_tall_step_behind_an_unwalkable_face_is_climbed() {
    const P: [(f32, f32); 4] = [(-2.0, -0.02), (0.28, -0.02), (0.694, 0.91), (3.0, 0.91)];
    let frames = walk_profile(&P, -0.02, 8);
    assert!(frames.last().expect("frames").0 > 0.88, "{frames:?}");
    climbs_monotonically(&frames);
    assert!(frames[0].0 < 0.3, "{frames:?}");
}

#[test]
fn an_exactly_vertical_riser_still_certifies() {
    const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.30, 0.0), (0.30, 0.40), (3.0, 1.48)];
    assert!(walk_profile(&P, 0.0, 6).last().expect("frames").0 > 0.40);
}

#[test]
fn stepping_off_a_kerb_follows_the_surface_down() {
    let frames = walk_from(
        world_from_profile(&KERB),
        Vec3::new(1.6, 0.28, 0.0),
        Vec3::NEG_X,
        8,
    );
    for (i, f) in frames.iter().enumerate() {
        assert!(f.2 || f.3, "frame {i} left the surface: {frames:?}");
    }
    assert!(frames.last().expect("frames").0 < 0.05, "{frames:?}");
}

#[test]
fn a_face_steeper_than_the_cone_still_descends_what_it_can() {
    const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.60, 0.0), (1.0, -0.91), (3.0, -0.91)];
    let frames = walk_from(
        world_from_profile(&P),
        Vec3::new(-0.6, 0.0, 0.0),
        Vec3::X,
        5,
    );
    let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO;
    let biggest = frames
        .windows(2)
        .map(|w| w[0].0 - w[1].0)
        .fold(frames[0].0.abs(), f32::max);
    assert!(biggest > cone * 0.5, "{frames:?}");
}

#[test]
fn a_ledge_deeper_than_the_probe_is_a_fall_not_an_absorbed_step() {
    const P: [(f32, f32); 4] = [(-2.0, 0.0), (0.60, 0.0), (0.62, -1.20), (3.0, -1.20)];
    let frames = walk_from(
        world_from_profile(&P),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::X,
        12,
    );
    let bound = 2.0 * (TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK);
    let worst = frames
        .windows(2)
        .map(|w| w[0].0 - w[1].0)
        .fold(0.0_f32, f32::max);
    assert!(worst <= bound, "worst drop {worst:.3}: {frames:#?}");
}

#[test]
fn an_idle_body_is_not_pulled_down_through_open_air() {
    let (dy, ground) = world_from_profile(&[(-2.0, 0.0), (3.0, 0.0)])
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let center = Vec3::new(0.0, CAPSULE_HEIGHT * 0.5 + 1.0, 0.0);
            let g = grounded_step(
                &world,
                &player_capsule(),
                center,
                Vec3::ZERO,
                dt,
                Support::default(),
            );
            (g.center.y - center.y, g.ground.is_some())
        })
        .expect("the system runs");
    assert!(!ground);
    assert!(dy.abs() <= STEP_SNAP_SLACK * 2.0, "dy={dy:+.3}");
}

/// A tavern table: its slab front contacts above the foot cone, so it pops.
#[test]
fn a_step_up_never_outruns_the_frame_it_happens_in() {
    const P: [(f32, f32); 6] = [
        (-2.0, 0.0),
        (0.95, 0.0),
        (0.95, 0.62),
        (0.60, 0.62),
        (0.60, 0.737),
        (3.0, 0.737),
    ];
    let track = world_from_profile(&P)
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let mut center = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5, 0.0);
            let mut support = Support::default();
            let mut rows = Vec::new();
            for _ in 0..24 {
                let g = grounded_step(
                    &world,
                    &player_capsule(),
                    center,
                    Vec3::X * 7.0,
                    dt,
                    support,
                );
                rows.push((g.center.x - center.x, g.center.y - CAPSULE_HEIGHT * 0.5));
                center = g.center;
                support.steep = g.steep_support;
            }
            rows
        })
        .expect("the system runs");
    let worst = track.iter().map(|r| r.0).fold(0.0_f32, f32::max);
    assert!(worst <= TRAVEL_60FPS + 1.0e-3, "{track:#?}");
    assert!(track.last().expect("rows").1 > 0.70, "{track:#?}");
}

#[test]
fn stepping_off_a_fence_hugs_the_edge_down_instead_of_dropping_at_once() {
    const P: [(f32, f32); 5] = [
        (-2.0, 0.72),
        (0.60, 0.72),
        (0.69, 0.60),
        (0.692, 0.0),
        (3.0, 0.0),
    ];
    let frames = walk_from(
        world_from_profile(&P),
        Vec3::new(-1.0, 0.72, 0.0),
        Vec3::X,
        14,
    );
    let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    let worst = frames
        .windows(2)
        .map(|w| w[0].0 - w[1].0)
        .fold(0.0_f32, f32::max);
    assert!(worst <= cone + 1.0e-3, "{frames:#?}");
    assert!(
        frames.iter().all(|&(_, _, steep, walk)| steep || walk),
        "{frames:#?}"
    );
    assert!(frames.last().expect("frames").0 < 0.05, "{frames:#?}");
}

#[test]
fn a_frame_that_finds_nothing_still_spends_only_one_cone() {
    let dy = world_from_profile(&[(-2.0, 0.0), (3.0, 0.0)])
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let center = Vec3::new(0.0, CAPSULE_HEIGHT * 0.5 + 1.6, 0.0);
            let support = Support {
                steep: true,
                ..Support::default()
            };
            let g = grounded_step(
                &world,
                &player_capsule(),
                center,
                Vec3::X * 7.0,
                dt,
                support,
            );
            g.center.y - g.unsupported.unwrap_or(0.0) - center.y
        })
        .expect("the system runs");
    let cone = TRAVEL_60FPS * STEP_SLOPE_RATIO + STEP_SNAP_SLACK;
    assert!(-dy <= cone + 1.0e-3, "spent {:.3}", -dy);
}

#[test]
fn a_motionless_body_is_never_perched_on_a_steep_slope() {
    const P: [(f32, f32); 3] = [(-2.0, 0.0), (0.0, 0.0), (3.0, -5.196)];
    let rows = world_from_profile(&P)
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let mut center = Vec3::new(0.6, CAPSULE_HEIGHT * 0.5 - 1.039, 0.0);
            let mut support = Support::default();
            (0..4)
                .map(|i| {
                    let v = if i < 2 { Vec3::X * 7.0 } else { Vec3::ZERO };
                    let g = grounded_step(&world, &player_capsule(), center, v, dt, support);
                    center = g.center;
                    support.steep = g.steep_support;
                    (v.length() > 0.0, g.steep_support)
                })
                .collect::<Vec<_>>()
        })
        .expect("the system runs");
    assert!(rows.iter().all(|&(moving, held)| moving || !held));
}

#[test]
fn the_kerb_is_ridden_up_its_skirt_never_popped() {
    let frames = walk_profile(&KERB, 0.0, 4);
    let arrived = frames
        .iter()
        .position(|&(y, ..)| (y - 0.28).abs() < 0.03)
        .expect("onto the tread");
    assert!(
        arrived > 0,
        "arriving on the first frame is a pop: {frames:?}"
    );
    assert!(frames[0].2, "{frames:?}");
    assert!(frames[0].0 > 0.0 && frames[0].0 < 0.28, "{frames:?}");
}

#[test]
fn a_wall_is_never_ridden() {
    for (y, climb, riding, _) in walk_profile(&KERB, -2.0, 4) {
        assert!(!riding && climb.is_none() && y < -2.0 + 0.05);
    }
}

#[test]
fn the_kerb_is_out_of_reach_of_one_frames_travel() {
    assert!(matches!(step_at(TRAVEL_60FPS), StepVerdict::SteepFloor));
}

#[test]
fn a_body_scaled_advance_climbs_the_kerb() {
    let StepVerdict::Commit { landed, .. } = step_at(STEP_UP_ADVANCE) else {
        panic!("the advance should reach the tread");
    };
    let rise = landed.y - CAPSULE_HEIGHT * 0.5;
    assert!((rise - 0.28).abs() < 0.03, "{rise:+.3}");
}

#[test]
fn the_advance_never_climbs_past_the_rise_ceiling() {
    let v = world_from_profile(&KERB)
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let capsule = player_capsule();
            let cast = |from: Vec3, disp: Vec3| world.cast_body(&capsule, from, disp, SKIN_WIDTH);
            let start = Vec3::new(-1.0, CAPSULE_HEIGHT * 0.5 - 2.0, 0.0);
            let run = cast(start, Vec3::X).map_or(1.0, |h| h.distance);
            step_up(
                &cast,
                start + Vec3::X * run,
                Vec3::X,
                TRAVEL_60FPS,
                STEP_UP_ADVANCE,
                STEP_UP_HEIGHT,
            )
            .verdict
        })
        .expect("the system runs");
    assert!(!matches!(v, StepVerdict::Commit { .. }), "{v:?}");
}

/// A 55° hillside.
#[test]
fn a_jump_into_a_steep_hillside_banks_no_height() {
    const P: [(f32, f32); 3] = [(-3.0, 0.0), (0.0, 0.0), (2.5, 3.570)];
    let arc = world_from_profile(&P)
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let dt = Duration::from_secs_f32(TRAVEL_60FPS / 7.0);
            let secs = dt.as_secs_f32();
            let mut center = Vec3::new(-0.6, CAPSULE_HEIGHT * 0.5, 0.0);
            let mut vel_y = JUMP_SPEED;
            (0..150)
                .map(|_| {
                    vel_y = (vel_y - GRAVITY * secs).max(-TERMINAL_VELOCITY);
                    let before = center.y;
                    center = airborne_step(
                        &world,
                        &player_capsule(),
                        center,
                        Vec3::X * 7.0 + Vec3::Y * vel_y,
                        dt,
                    );
                    let intent = vel_y * secs;
                    let frac = if intent < 0.0 {
                        (center.y - before) / intent
                    } else {
                        1.0
                    };
                    (center.y - CAPSULE_HEIGHT * 0.5, vel_y, frac)
                })
                .collect::<Vec<_>>()
        })
        .expect("the system runs");
    let peak = arc.iter().map(|r| r.0).fold(f32::MIN, f32::max);
    assert!(peak > 1.0, "{peak}");
    assert!(
        arc.last().expect("frames").0 < 0.05,
        "banked height: {arc:?}"
    );
    let worst = arc
        .iter()
        .filter(|r| r.1 < -WEDGE_MIN_FALL && r.0 > 0.2)
        .map(|r| r.2)
        .fold(f32::MAX, f32::min);
    assert!(
        worst > 0.98,
        "a falling frame kept only {worst:.2} of its descent"
    );
}

fn stepped(profile: &[(f32, f32)], script: &[(bool, Vec3, bool)]) -> (Player, Vec<Outcome>) {
    let script = script.to_vec();
    world_from_profile(profile)
        .world_mut()
        .run_system_once(move |world: WorldCollision<'_, '_>| {
            let time = frame_time(60.0);
            let mut player = Player::default();
            let outcomes = script
                .iter()
                .map(|&(moving, dir, jump)| {
                    step(
                        &mut player,
                        &time,
                        &world,
                        &player_capsule(),
                        moving,
                        dir,
                        7.0,
                        jump,
                    )
                })
                .collect();
            (player, outcomes)
        })
        .expect("the system runs")
}

const FLAT: [(f32, f32); 2] = [(-3.0, 0.0), (3.0, 0.0)];

#[test]
fn a_standstill_jump_steers_once_at_the_walk_speed() {
    let (player, outcomes) = stepped(
        &FLAT,
        &[
            (false, Vec3::ZERO, true),
            (true, Vec3::NEG_X, false),
            (true, Vec3::NEG_Z, false),
        ],
    );
    assert!(outcomes[0].jumped && outcomes[1].air_nudged && !outcomes[2].air_nudged);
    assert!((player.horiz_vel.length() - AIR_NUDGE_SPEED).abs() < 1.0e-4);
    assert!(player.horiz_vel.x < 0.0 && player.horiz_vel.z.abs() < 1.0e-4);
}

#[test]
fn a_moving_jump_keeps_its_momentum_locked() {
    let (player, outcomes) = stepped(&FLAT, &[(true, Vec3::X, true), (true, Vec3::NEG_X, false)]);
    assert!(outcomes[0].jumped && !outcomes[1].air_nudged);
    assert!((player.horiz_vel - Vec3::X * 7.0).length() < 1.0e-4);
}

#[test]
fn the_settle_holds_the_body_with_gravity_off() {
    let mut app = world_from_profile(&FLAT);
    let pos = app
        .world_mut()
        .run_system_once(|world: WorldCollision<'_, '_>| {
            let time = frame_time(60.0);
            let mut player = Player {
                pos: Vec3::new(0.0, 5.0, 0.0),
                settling: true,
                ..Player::default()
            };
            for _ in 0..30 {
                step(
                    &mut player,
                    &time,
                    &world,
                    &player_capsule(),
                    true,
                    Vec3::X,
                    7.0,
                    true,
                );
            }
            player.pos
        })
        .expect("the system runs");
    assert_eq!(pos, Vec3::new(0.0, 5.0, 0.0));
}
