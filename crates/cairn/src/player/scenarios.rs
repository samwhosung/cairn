//! The body walked through the real install, headless, one fixed step at a time, by the same
//! plugins and keys the window uses. Every scenario skips without `WOW_DATA`.

mod alone;
mod clock;
mod fight;
mod heard;
mod honest;
mod liar;
mod moved;
mod notes;
mod painter;
mod pair;
mod pictures;
mod solo;
mod together;
mod walker;

use avian3d::prelude::PhysicsLayer;
use bevy::input::keyboard::KeyCode;
use bevy::math::Vec3;
use bevy::prelude::{Transform, With};
use world::collision::CollisionLayer;
use world::unit::{CharacterLook, StandState};

use super::PlayerBody;
use super::flags::{FALLING, FORWARD, SWIMMING};
use super::state::{
    CAPSULE_HEIGHT, CAPSULE_RADIUS, GRAVITY, JUMP_SPEED, RUN_SPEED, SKIN_WIDTH, TERMINAL_VELOCITY,
};
use super::swim::{SWIM_SPEED, swim_enter_depth, swim_exit_depth};
use walker::{Frame, Walker};

/// Open meadow south of Goldshire, within half a yard of level and clear for 70 yards east and
/// north.
const MEADOW: [f32; 2] = [-9585.0, 90.0];

fn horizontal(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn run_ten_seconds(heading_deg: f32) -> Option<f32> {
    let mut w = Walker::on_ground(MEADOW, heading_deg, 60.0)?;
    let start = w.wow();
    w.press(KeyCode::KeyW);
    let frames = w.run(600);
    let covered = horizontal(start, frames[frames.len() - 1].wow);
    eprintln!("run at {heading_deg}°: {covered:.3} yd in 10 s from {start:?}");
    Some(covered)
}

/// East, along a coordinate near zero, the run is exact. North, along one near 9585, a position
/// holds only 1/1024 yd, so every frame's 7/60 yd lands on the nearest 1/1024.
#[test]
fn ten_seconds_at_a_run_covers_seventy_yards_of_meadow() {
    let step = RUN_SPEED / 60.0;
    let Some(east) = run_ten_seconds(270.0) else {
        return;
    };
    assert!((east - 10.0 * RUN_SPEED).abs() <= step, "{east}");
    let quantum = 1.0 / 1024.0;
    let north = run_ten_seconds(0.0).expect("the install");
    let quantised = 600.0 * (step / quantum).round() * quantum;
    assert!(
        (north - quantised).abs() <= step,
        "{north} against {quantised}"
    );
}

/// The feet rest on the ground under them, no deeper and no higher than the cast's skin holds
/// a capsule over a slope.
fn assert_resting(w: &mut Walker) {
    let [x, y, z] = w.wow();
    let (dist, n) = w
        .ray([x, y, z + 1.0], [0.0, 0.0, -1.0], 10.0)
        .expect("ground");
    let gap = dist - 1.0;
    let most = SKIN_WIDTH / n[2] + CAPSULE_RADIUS * (1.0 / n[2] - 1.0);
    eprintln!(
        "  rests {gap:+.4} over ground sloped {:.1}°",
        n[2].acos().to_degrees()
    );
    assert!((-1e-3..=most + 1e-3).contains(&gap), "{gap} against {most}");
}

#[test]
fn a_jump_peaks_at_the_analytic_apex_and_lands_on_time() {
    for hz in [30.0_f32, 60.0, 144.0] {
        let Some(mut w) = Walker::on_ground(MEADOW, 0.0, hz) else {
            return;
        };
        let ground = w.wow()[2];
        let mut frames = vec![w.tap(KeyCode::Space)];
        frames.extend(w.run((1.2 * hz) as usize));
        let apex = frames.iter().map(|f| f.wow[2]).fold(f32::MIN, f32::max) - ground;
        let airborne = frames.iter().filter(|f| f.flags & FALLING != 0).count() as f32 / hz;
        let (want_apex, want_air) = (
            JUMP_SPEED * JUMP_SPEED / (2.0 * GRAVITY),
            2.0 * JUMP_SPEED / GRAVITY,
        );
        eprintln!(
            "jump at {hz} Hz: apex {apex:.4} yd (want {want_apex:.4}), \
             airborne {airborne:.4} s (want {want_air:.4})"
        );
        assert!(
            (apex - want_apex).abs() <= 0.5 * GRAVITY / (hz * hz) + 0.01,
            "{apex}"
        );
        assert!((airborne - want_air).abs() <= 1.0 / hz + 1e-4, "{airborne}");
        assert_resting(&mut w);
    }
}

fn fall_from(drop: f32, hz: f32) -> Option<(f32, f32)> {
    let mut w = Walker::on_ground(MEADOW, 0.0, hz)?;
    let ground = w.wow()[2];
    w.teleport(Vec3::new(MEADOW[0], MEADOW[1], ground + drop));
    let frames: Vec<Frame> = w.run((5.0 * hz) as usize);
    let landing = frames
        .iter()
        .position(|f| f.flags & FALLING == 0)
        .expect("the fall lands");
    let impact = -frames[..landing]
        .iter()
        .map(|f| f.vel_y)
        .fold(0.0, f32::min);
    assert_resting(&mut w);
    Some((landing as f32 / hz, impact))
}

#[test]
fn a_fall_takes_the_time_gravity_and_the_terminal_speed_say() {
    for drop in [30.0_f32, 150.0] {
        let Some((t, impact)) = fall_from(drop, 60.0) else {
            return;
        };
        let t_terminal = TERMINAL_VELOCITY / GRAVITY;
        let d_terminal = TERMINAL_VELOCITY * t_terminal / 2.0;
        let (want_t, want_v) = if drop <= d_terminal {
            ((2.0 * drop / GRAVITY).sqrt(), (2.0 * GRAVITY * drop).sqrt())
        } else {
            (
                t_terminal + (drop - d_terminal) / TERMINAL_VELOCITY,
                TERMINAL_VELOCITY,
            )
        };
        eprintln!(
            "fall {drop} yd: lands after {t:.4} s (want {want_t:.4}) at {impact:.3} yd/s \
             (want {want_v:.3})"
        );
        assert!((t - want_t).abs() <= 1.0 / 60.0 + 1e-4, "{t}");
        assert!((impact - want_v).abs() <= GRAVITY / 60.0, "{impact}");
    }
}

/// Below the 55° face of an Elwynn hillside, a 41° bank rising 1.5 yd to a shelf at 100.24:
/// uphill is 311.9°.
const HILLSIDE: [f32; 2] = [-9240.1, -338.13];

#[test]
fn a_bank_under_fifty_degrees_is_walked_and_a_face_over_it_refused() {
    let Some(mut w) = Walker::on_ground(HILLSIDE, 311.9, 60.0) else {
        return;
    };
    let start = w.wow();
    w.press(KeyCode::KeyW);
    let frames = w.run(300);
    let (peak, top) =
        frames
            .iter()
            .map(|f| f.wow[2])
            .enumerate()
            .fold(
                (0, f32::MIN),
                |best, (i, z)| if z > best.1 { (i, z) } else { best },
            );
    eprintln!(
        "hillside: from {:.3} up to {top:.3} after {peak} frames",
        start[2]
    );
    assert!(top - start[2] > 1.9, "the bank is climbed: {top}");
    assert!(top < 100.55, "never half a yard onto the face: {top}");
    assert!(
        frames[peak..].iter().all(|f| f.wow[2] <= top),
        "no ratchet up the face"
    );
}

/// A block `h` tall across the meadow three yards east of `start`: `(its top, the highest the
/// feet got and where they ended, after running at it for a second and a half)`.
fn run_at_ledge(w: &mut Walker, start: [f32; 3], h: f32) -> (f32, f32, [f32; 3]) {
    let face_y = start[1] - 3.0;
    let ground = w
        .ground_under(start[0], face_y + 0.2, start[2] + 5.0)
        .expect("ground");
    let block = w.block(
        [start[0] - 2.0, face_y - 3.0, ground - 3.0],
        [start[0] + 2.0, face_y, ground + h],
    );
    w.teleport(Vec3::from_array(start));
    w.aim(270.0);
    w.press(KeyCode::KeyW);
    let frames = w.run(90);
    w.release(KeyCode::KeyW);
    w.remove(block);
    let high = frames.iter().map(|f| f.wow[2]).fold(f32::MIN, f32::max);
    (ground + h, high, frames[frames.len() - 1].wow)
}

#[test]
fn a_one_yard_ledge_is_stepped_up_and_a_yard_and_a_half_is_not() {
    let Some(mut w) = Walker::on_ground(MEADOW, 270.0, 60.0) else {
        return;
    };
    let start = w.wow();
    let (top, high, end) = run_at_ledge(&mut w, start, 1.0);
    eprintln!("ledge 1.0: top {top:.3}, feet up to {high:.3}, ended {end:?}");
    assert!(
        (high - top).abs() < SKIN_WIDTH,
        "onto the block: {high} against {top}"
    );
    let (top, high, end) = run_at_ledge(&mut w, start, 1.5);
    eprintln!("ledge 1.5: top {top:.3}, feet up to {high:.3}, ended {end:?}");
    assert!(high < start[2] + 0.05, "not onto the block: {high}");
    assert!(
        end[1] > start[1] - 3.0 + CAPSULE_RADIUS,
        "stopped at its face: {end:?}"
    );
}

/// Holds W and aims at each waypoint in turn, as a player steering with the mouse: the frames,
/// and whether the last was reached within `secs`.
fn walk_path(w: &mut Walker, path: &[[f32; 2]], hz: f32, secs: f32) -> (Vec<Frame>, bool) {
    let mut frames = Vec::new();
    let mut next = 0;
    w.press(KeyCode::KeyW);
    for _ in 0..(secs * hz) as usize {
        let [x, y, _] = w.wow();
        while next < path.len() && (path[next][0] - x).hypot(path[next][1] - y) < 0.25 {
            next += 1;
        }
        if next == path.len() {
            break;
        }
        w.aim((path[next][1] - y).atan2(path[next][0] - x).to_degrees());
        frames.extend(w.run(1));
    }
    w.release(KeyCode::KeyW);
    (frames, next == path.len())
}

/// Up the abbey's west stairs from the hall floor at 81.94: a flight to the landing at 85.56, a
/// turn, a flight to the gallery at 89.17.
const ABBEY_STAIRS: [[f32; 2]; 4] = [
    [-8908.8, -200.5],
    [-8906.0, -200.6],
    [-8905.3, -193.5],
    [-8905.0, -191.0],
];
const GALLERY: f32 = 89.166;

#[test]
fn the_abbey_stairs_are_walked_to_the_top() {
    let Some(mut w) = Walker::new("Azeroth", [-8908.6, -190.5, 82.5], 270.0, 60.0) else {
        return;
    };
    let (frames, arrived) = walk_path(&mut w, &ABBEY_STAIRS, 60.0, 8.0);
    let airborne = frames.iter().filter(|f| f.flags & FALLING != 0).count();
    let end = w.wow();
    eprintln!(
        "stairs: arrived {arrived} after {:.2} s, {airborne} frames airborne, at {end:?}",
        frames.len() as f32 / 60.0
    );
    assert!(arrived && frames.len() < 4 * 60, "{} frames", frames.len());
    assert!(
        (0.0..SKIN_WIDTH).contains(&(end[2] - GALLERY)),
        "on the gallery: {end:?}"
    );
}

/// Crystal Lake's north shore: the bed shelves to swimming depth about six yards out.
const SHORE: [f32; 2] = [-9414.0, -316.0];

/// A frame's latch reads the depth the previous frame left, hence `first - 1`.
#[test]
fn deep_water_is_swum_into_and_out_of() {
    let Some(mut w) = Walker::on_ground(SHORE, 180.0, 60.0) else {
        return;
    };
    w.press(KeyCode::KeyW);
    let mut frames = w.run(300);
    w.aim(0.0);
    frames.extend(w.run(420));
    let depth: Vec<Option<f32>> = frames
        .iter()
        .map(|f| w.water(f.wow).map(|s| s - f.wow[2]))
        .collect();
    let swims = |i: usize| frames[i].flags & SWIMMING != 0;
    let (enter, exit) = (
        swim_enter_depth(CAPSULE_HEIGHT),
        swim_exit_depth(CAPSULE_HEIGHT),
    );
    let first = (0..frames.len()).find(|&i| swims(i)).expect("a swim");
    let last = (first..frames.len())
        .find(|&i| !swims(i))
        .expect("out again")
        - 1;
    let at = |i: usize| depth[i].expect("in the water");
    eprintln!(
        "swim: frames {first} to {last}, begun at depth {:.3} (over {enter:.3}), ended at {:.3} \
         (under {exit:.3})",
        at(first - 1),
        at(last)
    );
    assert!(at(first - 1) > enter && at(first - 2) <= enter);
    assert!(at(last) < exit && at(last - 1) >= exit);
    let (a, b) = (frames[first].wow, frames[299].wow);
    assert!((a[2] - b[2]).abs() < 1.0e-4, "the depth holds: {a:?} {b:?}");
    let speed = horizontal(a, b) / ((299 - first) as f32 / 60.0);
    eprintln!("swim speed {speed:.4} yd/s");
    assert!((speed - SWIM_SPEED).abs() <= 60.0 / 1024.0, "{speed}");
    let end = frames[frames.len() - 1];
    assert!(depth[frames.len() - 1].is_none() && end.flags & (SWIMMING | FALLING) == 0);
}

/// The top of a stone ramp beside a pier on Stormwind's canals, which runs down into the
/// building's own water; no terrain liquid covers it.
const CANAL_RAMP: [f32; 2] = [-8761.44, 527.36];
const DOWN_THE_RAMP: f32 = 212.6;
const CANAL_SURFACE_Z: f32 = 95.474;

/// The swimmer grazes the ramp, so neither its depth nor its speed is the open water's.
#[test]
fn a_stormwind_canal_is_swum_into_and_out_of() {
    let Some(mut w) = Walker::on_ground(CANAL_RAMP, DOWN_THE_RAMP, 60.0) else {
        return;
    };
    let start = w.wow();
    w.press(KeyCode::KeyW);
    let mut frames = w.run(120);
    for _ in 0..420 {
        let [x, y, _] = w.wow();
        w.aim((start[1] - y).atan2(start[0] - x).to_degrees());
        frames.extend(w.run(1));
    }
    let surface: Vec<Option<f32>> = frames.iter().map(|f| w.water(f.wow)).collect();
    let depth = |i: usize| surface[i].expect("in the canal") - frames[i].wow[2];
    let swims = |i: usize| frames[i].flags & SWIMMING != 0;
    let (enter, exit) = (
        swim_enter_depth(CAPSULE_HEIGHT),
        swim_exit_depth(CAPSULE_HEIGHT),
    );
    let first = (0..frames.len()).find(|&i| swims(i)).expect("a swim");
    let last = (first..frames.len())
        .find(|&i| !swims(i))
        .expect("out again")
        - 1;
    eprintln!(
        "canal swim: frames {first} to {last}, begun at depth {:.3} (over {enter:.3}), ended at \
         {:.3} (under {exit:.3})",
        depth(first - 1),
        depth(last)
    );
    assert!(depth(first - 1) > enter && depth(first - 2) <= enter);
    assert!(depth(last) < exit && depth(last - 1) >= exit);
    assert!(
        (first..=last).all(|i| surface[i].is_some_and(|z| (z - CANAL_SURFACE_Z).abs() < 1e-3)),
        "the building's water"
    );
    let end = frames[frames.len() - 1];
    assert!(end.flags & (SWIMMING | FALLING) == 0, "{:?}", end.wow);
}

#[test]
fn a_tauren_and_a_gnome_are_drawn_and_collide_at_their_size() {
    for (race, sex, scale, height) in [
        (6, 0, 1.35, 1.653 * 1.35),
        (6, 1, 1.25, 2.111 * 1.25),
        (7, 0, 1.15, 1.056 * 1.15),
        (7, 1, 1.15, 1.15),
    ] {
        let look = CharacterLook::naked(race, sex);
        let Some(mut w) = Walker::dressed_on_ground(MEADOW, 0.0, 60.0, look) else {
            return;
        };
        let world = w.app.world_mut();
        let drawn = world
            .query_filtered::<&Transform, With<PlayerBody>>()
            .single(world)
            .expect("the body")
            .scale;
        let h = w.player().collision_height;
        eprintln!("race {race} sex {sex}: drawn at {drawn}, collides {h:.4} yd tall");
        assert!((drawn - Vec3::splat(scale)).abs().max_element() < 1e-6);
        assert!((h - height).abs() < 1e-5, "{h} against {height}");
    }
}

#[test]
fn x_sits_the_body_down_and_walking_stands_it_up() {
    let Some(mut w) = Walker::on_ground(MEADOW, 0.0, 60.0) else {
        return;
    };
    w.tap(KeyCode::KeyX);
    w.run(30);
    assert_eq!(w.player().stand_state, StandState::SIT, "seated");
    w.press(KeyCode::KeyW);
    let moved = w.run(1)[0];
    let standing = StandState::STAND;
    assert_eq!(w.player().stand_state, standing, "the first step stands it");
    assert!(moved.flags & FORWARD != 0);
    w.tap(KeyCode::KeyX);
    assert_eq!(w.player().stand_state, standing, "no sitting on the run");
    w.release(KeyCode::KeyW);
    w.run(2);
    w.tap(KeyCode::KeyX);
    w.tap(KeyCode::KeyX);
    assert_eq!(w.player().stand_state, standing, "X again stands it");
}

struct Wade {
    collision_height: f32,
    last_wading_depth: f32,
    depth_it_swims_at: f32,
}

fn wade_in(look: CharacterLook) -> Option<Wade> {
    let mut w = Walker::dressed_on_ground(SHORE, 180.0, 60.0, look)?;
    w.press(KeyCode::KeyW);
    let frames = w.run(600);
    let first = frames
        .iter()
        .position(|f| f.flags & SWIMMING != 0)
        .expect("a swim");
    let mut depth = |i: usize| {
        let f: Frame = frames[i];
        w.water(f.wow).expect("in the water") - f.wow[2]
    };
    let (last_wading_depth, depth_it_swims_at) = (depth(first - 2), depth(first - 1));
    Some(Wade {
        collision_height: w.player().collision_height,
        last_wading_depth,
        depth_it_swims_at,
    })
}

#[test]
fn a_tauren_wades_out_deeper_than_a_gnome_before_it_swims() {
    let mut depths = Vec::new();
    for (race, sex) in [(6, 1), (7, 1)] {
        let Some(wade) = wade_in(CharacterLook::naked(race, sex)) else {
            return;
        };
        let (h, before, at) = (
            wade.collision_height,
            wade.last_wading_depth,
            wade.depth_it_swims_at,
        );
        let enter = swim_enter_depth(h);
        eprintln!("race {race} sex {sex}: {h:.4} yd tall swims at {at:.3} (over {enter:.3})");
        assert!(
            before <= enter && at > enter,
            "{before} {at} against {enter}"
        );
        depths.push(at);
    }
    assert!(depths[0] > depths[1] + 1.0, "{depths:?}");
}

/// The Goldshire inn's north wall, a WMO face leaning 1.5° back from its base: its outward normal
/// and its plane a yard above the ground, `n · (x, y) = c`. It runs flat from y = 26 to y = 20.
const INN_WALL: ([f32; 2], f32) = ([0.992_55, -0.121_91], -9383.97);

const INN_WALL_START: [f32; 2] = {
    let ([nx, ny], c) = INN_WALL;
    let y = 25.5;
    [(c - ny * y) / nx + 2.0 * nx, y + 2.0 * ny]
};

/// At 240 Hz each step along the wall is under 3/1024 yd in x, which a position this far from the
/// origin rounds into the wall until the cast meets it at zero distance: the slide stops there.
#[test]
fn a_wmo_wall_is_slid_along_and_never_tunnelled_at_any_frame_rate() {
    let ([nx, ny], _) = INN_WALL;
    let start = INN_WALL_START;
    for hz in [20.0_f32, 30.0, 60.0, 120.0, 144.0, 240.0] {
        let Some(mut w) = Walker::on_ground(start, 218.0, hz) else {
            return;
        };
        w.press(KeyCode::KeyW);
        let mut nearest = f32::MAX;
        for _ in 0..(1.2 * hz) as usize {
            let [x, y, z] = w.run(1)[0].wow;
            let (d, _, layers) = w
                .ray_hit([x, y, z + 1.0], [-nx, -ny, 0.0], 5.0)
                .expect("the wall, in front");
            assert_eq!(layers, CollisionLayer::Walk.to_bits(), "a WMO face");
            nearest = nearest.min(d);
        }
        let end = w.wow();
        eprintln!(
            "wall at {hz} Hz: nearest {nearest:.4}, ended at y {:.3}",
            end[1]
        );
        assert!(nearest > CAPSULE_RADIUS - 0.01, "{nearest}");
        let reached = if hz < 240.0 { 20.2 } else { 23.0 };
        assert!(end[1] < reached, "{end:?}");
    }
}

/// The ends of a fence north-east of Goldshire that stands across the border between two tiles,
/// so both tiles place it.
const BORDER_FENCE: [[f32; 2]; 2] = [[-9400.34, -2.10], [-9403.73, 0.70]];

const PAST_THE_BORDER: f32 = 515.0;

fn fence_stops(w: &mut Walker) -> bool {
    let [a, b] = BORDER_FENCE;
    let mid = [f32::midpoint(a[0], b[0]), f32::midpoint(a[1], b[1])];
    let length = (b[0] - a[0]).hypot(b[1] - a[1]);
    let normal = [(b[1] - a[1]) / length, (a[0] - b[0]) / length];
    let start = [mid[0] + 3.0 * normal[0], mid[1] + 3.0 * normal[1]];
    w.teleport(Vec3::new(start[0], start[1], 500.0));
    let ground = w
        .ground_under(start[0], start[1], 500.0)
        .expect("ground by the fence");
    w.teleport(Vec3::new(start[0], start[1], ground));
    w.aim((-normal[1]).atan2(-normal[0]).to_degrees());
    w.press(KeyCode::KeyW);
    w.run(90);
    w.release(KeyCode::KeyW);
    let end = w.wow();
    let stop = (end[0] - mid[0]) * normal[0] + (end[1] - mid[1]) * normal[1];
    (0.0..2.0 * CAPSULE_RADIUS).contains(&stop)
}

#[test]
fn a_fence_on_a_tile_border_stands_after_either_tile_leaves() {
    let Some(mut w) = Walker::on_ground(GOLDSHIRE, 0.0, 60.0) else {
        return;
    };
    assert!(fence_stops(&mut w), "the fence stops a body at first");
    for side in [PAST_THE_BORDER, -PAST_THE_BORDER, PAST_THE_BORDER] {
        let x = BORDER_FENCE[0][0];
        w.teleport(Vec3::new(x, side, 500.0));
        assert!(
            w.ground_under(x, side / 2.0, 500.0).is_some(),
            "the near tile stays"
        );
        assert!(
            w.ground_under(x, -side / 2.0, 500.0).is_none(),
            "the far tile has left"
        );
        assert!(
            fence_stops(&mut w),
            "the fence did not stop the body once it had been to y {side}"
        );
    }
}

fn trimeshes(w: &mut Walker) -> Vec<u64> {
    use std::hash::{DefaultHasher, Hash, Hasher};

    use avian3d::prelude::Collider;

    let world = w.app.world_mut();
    let mut fingerprints: Vec<u64> = world
        .query::<&Collider>()
        .iter(world)
        .filter_map(|c| c.shape().as_trimesh())
        .map(|mesh| {
            let mut hasher = DefaultHasher::new();
            for v in mesh.vertices() {
                [v.x, v.y, v.z].map(f32::to_bits).hash(&mut hasher);
            }
            mesh.indices().hash(&mut hasher);
            hasher.finish()
        })
        .collect();
    fingerprints.sort_unstable();
    fingerprints
}

#[test]
fn hops_a_frame_apart_leave_the_colliders_that_settled_hops_do() {
    let hops = [PAST_THE_BORDER, 0.0, -PAST_THE_BORDER, 0.0, PAST_THE_BORDER]
        .map(|y| Vec3::new(BORDER_FENCE[0][0], y, 500.0));
    let Some(mut settled) = Walker::on_ground(GOLDSHIRE, 0.0, 60.0) else {
        return;
    };
    for at in hops {
        settled.teleport(at);
    }
    let mut brisk = Walker::on_ground(GOLDSHIRE, 0.0, 60.0).expect("a second walker");
    for at in hops {
        brisk.hop(at);
    }
    brisk.settle();
    assert_eq!(trimeshes(&mut settled), trimeshes(&mut brisk));
}

/// Goldshire's crossroads, north of the inn.
const GOLDSHIRE: [f32; 2] = [-9450.0, 60.0];

/// What a frame costs the main thread with the collision world streamed around Goldshire: the
/// settle from nothing, then a minute running east out of the village, over a tile edge.
#[test]
#[ignore = "a measurement, for a release build"]
fn the_frame_cost_of_walking_goldshire() {
    use std::time::{Duration, Instant};

    use avian3d::prelude::Collider;
    use bevy::prelude::With;

    let begun = Instant::now();
    let Some(mut w) = Walker::on_ground(GOLDSHIRE, 270.0, 60.0) else {
        return;
    };
    let settle = begun.elapsed();
    w.press(KeyCode::KeyW);
    let mut costs: Vec<Duration> = (0..3600)
        .map(|_| {
            let t = Instant::now();
            w.run(1);
            t.elapsed()
        })
        .collect();
    let world = w.app.world_mut();
    let colliders = world
        .query_filtered::<&Collider, With<avian3d::prelude::RigidBody>>()
        .iter(world)
        .filter_map(|c| c.shape_scaled().as_trimesh().map(|t| t.indices().len()))
        .fold((0, 0), |(n, tris), t| (n + 1, tris + t));
    let over = |ms: u64| {
        costs
            .iter()
            .filter(|c| **c > Duration::from_millis(ms))
            .count()
    };
    let (over1, over2) = (over(1), over(2));
    costs.sort();
    let at = |q: f32| costs[((costs.len() - 1) as f32 * q) as usize].as_secs_f64() * 1e3;
    let mean = costs.iter().sum::<Duration>().as_secs_f64() * 1e3 / costs.len() as f64;
    eprintln!(
        "goldshire: settled in {:.2} s; {} trimesh colliders, {} triangles; per frame mean \
         {mean:.3} ms, p50 {:.3}, p90 {:.3}, p99 {:.3}, max {:.3}, {over1} over 1 ms and {over2} \
         over 2; ran to {:?}",
        settle.as_secs_f64(),
        colliders.0,
        colliders.1,
        at(0.5),
        at(0.9),
        at(0.99),
        at(1.0),
        w.wow()
    );
}
