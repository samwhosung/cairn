use bevy::input::keyboard::KeyCode;
use bevy::math::Vec3;
use world::{Filed, Install, PlacementEdits};

use super::MEADOW;
use super::walker::Walker;
use crate::player::state::{CAPSULE_HEIGHT, CAPSULE_RADIUS, RUN_SPEED, SKIN_WIDTH};

const GOLDSHIRE_HOUSE: u32 = 54_705;
const EAST_DEG: f32 = 270.0;
const FRAMES_A_SECOND: f32 = 60.0;
const RUN_FRAMES: usize = 180;
const HALF_SECOND_FRAMES: usize = 30;
const RUN_PER_FRAME_YD: f32 = RUN_SPEED / FRAMES_A_SECOND;
const HOUSE_MOVED_EAST_YD: f32 = 16.0;
const SQUARE_WITH_THE_RUN_HEADING_DEG: f32 = 90.0;
const TREE_MOVED_EAST_YD: f32 = 12.0;
const TRUNK_PROBE_WEST_YD: f32 = 10.0;
const TRUNK_PROBE_UP_YD: f32 = 3.0;

fn tiles_round_the_meadow(install: &Install) -> Vec<terrain::TileMesh> {
    let (x, y) = wdt::world_to_tile(MEADOW[0], MEADOW[1]);
    (x.saturating_sub(1)..=x + 1)
        .flat_map(|tx| (y.saturating_sub(1)..=y + 1).map(move |ty| (tx, ty)))
        .filter_map(|(tx, ty)| {
            let adt = format!("World\\Maps\\Azeroth\\Azeroth_{tx}_{ty}.adt");
            terrain::adt_to_tile_mesh(&install.0.read(&adt).ok()?).ok()
        })
        .collect()
}

fn goldshire_house(install: &Install) -> Filed {
    tiles_round_the_meadow(install)
        .into_iter()
        .flat_map(|tile| tile.wmos)
        .find(|w| w.unique_id == GOLDSHIRE_HOUSE)
        .map(Filed::Building)
        .expect("Goldshire's house in a tile by the meadow")
}

fn tree_nearest_the_meadow(install: &Install) -> Filed {
    let away = |p: [f32; 3]| (p[0] - MEADOW[0]).hypot(p[1] - MEADOW[1]);
    tiles_round_the_meadow(install)
        .into_iter()
        .flat_map(|tile| tile.doodads)
        .filter(|d| d.model.to_ascii_lowercase().contains("tree"))
        .min_by(|a, b| {
            away(a.position)
                .total_cmp(&away(b.position))
                .then(a.unique_id.cmp(&b.unique_id))
        })
        .map(Filed::Doodad)
        .expect("a tree by the meadow")
}

struct Run {
    east_yd: f32,
    last_half_second_east_yd: f32,
    end: [f32; 3],
}

fn run_east(w: &mut Walker, from: [f32; 3]) -> Run {
    w.teleport(Vec3::from_array(from));
    w.aim(EAST_DEG);
    w.press(KeyCode::KeyW);
    let frames = w.run(RUN_FRAMES);
    w.release(KeyCode::KeyW);
    let end = frames[frames.len() - 1].wow;
    let late = frames[frames.len() - 1 - HALF_SECOND_FRAMES].wow;
    Run {
        east_yd: from[1] - end[1],
        last_half_second_east_yd: late[1] - end[1],
        end,
    }
}

fn a_clear_run_yd() -> f32 {
    RUN_SPEED * RUN_FRAMES as f32 / FRAMES_A_SECOND
}

fn edit(w: &mut Walker, filed: Filed) {
    w.app
        .world_mut()
        .resource_mut::<PlacementEdits>()
        .place(filed);
    w.run(1);
    w.settle();
}

fn nearest_face_east_across_the_capsule(w: &mut Walker, feet: [f32; 3], reach: f32) -> Option<f32> {
    let side = CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS;
    (0..=10)
        .filter_map(|k| {
            let up = CAPSULE_RADIUS + side * k as f32 / 10.0;
            let from = [feet[0], feet[1], feet[2] + up];
            w.ray(from, [0.0, -1.0, 0.0], reach).map(|(d, _)| d)
        })
        .reduce(f32::min)
}

#[test]
fn a_house_moved_across_the_meadow_stops_the_run_at_its_wall_that_passed_before() {
    let Some(mut w) = Walker::on_ground(MEADOW, EAST_DEG, FRAMES_A_SECOND) else {
        return;
    };
    let start = w.wow();
    let before = run_east(&mut w, start);
    eprintln!(
        "before the move the run covers {:.3} yd of {:.3}",
        before.east_yd,
        a_clear_run_yd()
    );
    assert!(
        before.east_yd > a_clear_run_yd() - 0.5,
        "the meadow is clear"
    );
    assert!(
        before.east_yd > HOUSE_MOVED_EAST_YD,
        "and passes where the house goes"
    );

    let install = w.app.world().resource::<Install>().clone();
    let house = goldshire_house(&install);
    let at = [start[0], start[1] - HOUSE_MOVED_EAST_YD, start[2]];
    let square = [0.0, SQUARE_WITH_THE_RUN_HEADING_DEG, 0.0];
    let scale = house.scale();
    edit(&mut w, house.stood(at, square, scale));
    let after = run_east(&mut w, start);
    let wall = nearest_face_east_across_the_capsule(&mut w, after.end, a_clear_run_yd())
        .expect("the moved house stands in the way");
    eprintln!(
        "after the move the run stops {:.3} yd east, {:.3} yd up and {:.3} yd north, gaining \
         {:.4} yd east in its last half second, with a wall {wall:.3} yd ahead",
        after.east_yd,
        after.end[2] - start[2],
        after.end[0] - start[0],
        after.last_half_second_east_yd,
    );
    assert!(
        after.east_yd < HOUSE_MOVED_EAST_YD,
        "short of the house's middle"
    );
    assert!(after.last_half_second_east_yd < 0.01, "and stopped");
    let short = wall - CAPSULE_RADIUS;
    assert!(
        (0.0..=SKIN_WIDTH + RUN_PER_FRAME_YD).contains(&short),
        "against the wall: {short} yd short of touching it"
    );
}

fn probe_the_trunk(w: &mut Walker, trunk: [f32; 3]) -> Option<f32> {
    let from = [
        trunk[0],
        trunk[1] + TRUNK_PROBE_WEST_YD,
        trunk[2] + TRUNK_PROBE_UP_YD,
    ];
    w.ray(from, [0.0, -1.0, 0.0], 2.0 * TRUNK_PROBE_WEST_YD)
        .map(|(d, _)| d)
}

#[test]
fn a_tree_moved_into_the_run_holds_it_back_frees_its_old_place_and_moves_back() {
    let Some(mut w) = Walker::on_ground(MEADOW, EAST_DEG, FRAMES_A_SECOND) else {
        return;
    };
    let start = w.wow();
    let install = w.app.world().resource::<Install>().clone();
    let tree = tree_nearest_the_meadow(&install);
    let old = tree.position();
    let trunk_before = probe_the_trunk(&mut w, old);
    let before = run_east(&mut w, start);

    let new = [start[0], start[1] - TREE_MOVED_EAST_YD, start[2]];
    let (rotation, scale) = (tree.rotation(), tree.scale());
    edit(&mut w, tree.clone().stood(new, rotation, scale));
    let trunk_after = probe_the_trunk(&mut w, old);
    let after = run_east(&mut w, start);

    edit(&mut w, tree.clone());
    let back = run_east(&mut w, start);
    let trunk_back = probe_the_trunk(&mut w, old);
    eprintln!(
        "{} {}: its trunk met {trunk_before:?} yd off, then {trunk_after:?}, then {trunk_back:?}; \
         the run covers {:.3} yd, then {:.3}, then {:.3}",
        tree.unique_id(),
        tree.model(),
        before.east_yd,
        after.east_yd,
        back.east_yd,
    );
    let clear = a_clear_run_yd() - 0.5;
    assert!(
        trunk_before.is_some(),
        "the tree stands where its tile put it"
    );
    assert!(before.east_yd > clear, "the meadow is clear");
    assert!(trunk_after.is_none(), "moved, its old place is free");
    assert!(
        after.east_yd < TREE_MOVED_EAST_YD - 1.0,
        "its trunk holds the run back, which slides round it: {}",
        after.east_yd
    );
    assert!(back.east_yd > clear, "moved back, the run is clear");
    assert_eq!(
        trunk_back, trunk_before,
        "and its trunk stands where it stood"
    );
}
