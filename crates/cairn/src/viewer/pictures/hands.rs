use std::path::{Path, PathBuf};

use bevy::prelude::*;
use world::Filed;
use world::hands::Ghost;
use world::sight::frame::{Shown, SightIndex};

use super::{Lines, SIZE, Viewer, lamppost, load, pictures, pixels_differing, start};

const GRID_PX: usize = 32;
const BARREL: &str = "World\\Azeroth\\Westfall\\PassiveDoodads\\Barrel\\WestFallBarrel01.m2";
const UNUSED_UNIQUE_ID: u32 = 4_000_000_000;

#[derive(Debug, PartialEq, Eq)]
enum Named {
    Placement(u32),
    Ground,
    Nothing,
}

fn named(answer: &str) -> Named {
    match answer.split_whitespace().collect::<Vec<_>>()[..] {
        ["ok", "placement", id, ..] => Named::Placement(id.parse().expect("an id")),
        ["ok", "ground", ..] => Named::Ground,
        ["ok", "nothing"] => Named::Nothing,
        _ => panic!("no pick: {answer}"),
    }
}

struct SightNames {
    width: usize,
    names: Vec<Named>,
}

impl SightNames {
    fn read(frame: &Path, index: &SightIndex) -> Self {
        let image = load(frame);
        let width = image.width() as usize;
        let rgba = image.data.unwrap_or_default();
        let names = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| match index.shown([p[0], p[1], p[2]]) {
                Some(Shown::Placed(placed)) => Named::Placement(placed.unique_id),
                Some(Shown::Ground) => Named::Ground,
                Some(Shown::Nothing) | None => Named::Nothing,
            })
            .collect();
        Self { width, names }
    }

    fn height(&self) -> usize {
        self.names.len() / self.width
    }

    fn at(&self, x: usize, y: usize) -> &Named {
        &self.names[y * self.width + x]
    }

    fn nearest_ground_to(&self, x: usize, y: usize) -> (usize, usize) {
        let aim = y * self.width + x;
        let i = (0..self.names.len())
            .filter(|&i| self.names[i] == Named::Ground)
            .min_by_key(|&i| i.abs_diff(aim))
            .expect("ground on the frame");
        (i % self.width, i / self.width)
    }
}

fn on_frame(answer: &str) -> Rect {
    let at = answer
        .split("on the frame at ")
        .nth(1)
        .expect("on the frame");
    let numbers: Vec<f32> = at
        .split([',', ' ', ';'])
        .filter_map(|w| w.parse().ok())
        .take(4)
        .collect();
    Rect::new(numbers[0], numbers[1], numbers[2], numbers[3])
}

fn differing_outside(a: &Path, b: &Path, inside: &[Rect]) -> usize {
    let (a, b) = (load(a), load(b));
    let width = a.width() as usize;
    let (a, b) = (a.data.unwrap_or_default(), b.data.unwrap_or_default());
    let (a, b) = (a.as_chunks::<4>().0, b.as_chunks::<4>().0);
    a.iter()
        .zip(b)
        .enumerate()
        .filter(|(i, (p, q))| {
            let at = Vec2::new((i % width) as f32 + 0.5, (i / width) as f32 + 0.5);
            p[..3] != q[..3] && !inside.iter().any(|r| r.contains(at))
        })
        .count()
}

fn placed_at(answer: &str) -> String {
    let at = answer.split(" at ").nth(1).expect("a place");
    at.split_whitespace().next().expect("x,y,z").to_owned()
}

fn shoot(viewer: &mut Viewer, out: &Path, flags: &str) {
    let answer = viewer.ask(&format!("shot {} {flags}", out.display()));
    assert!(answer.starts_with("ok "), "{answer}");
}

fn picks_name_what_the_frame_shows(viewer: &mut Viewer, shown: &SightNames, bare: &SightNames) {
    let (mut picked, mut agree, mut inner) = (0, 0, 0);
    for y in (GRID_PX / 2..shown.height()).step_by(GRID_PX) {
        for x in (GRID_PX / 2..shown.width).step_by(GRID_PX) {
            let pick = named(&viewer.ask(&format!("pick {x} {y}")));
            let there = shown.at(x, y);
            let same_around = (y - 1..=y + 1)
                .flat_map(|v| (x - 1..=x + 1).map(move |u| (u, v)))
                .all(|(u, v)| shown.at(u, v) == there);
            picked += 1;
            agree += usize::from(pick == *there);
            if same_around {
                inner += 1;
                assert_eq!(pick, *there, "pixel {x},{y}, well inside what it shows");
            }
            if let Named::Placement(_) = there {
                let through = named(&viewer.ask(&format!("pick {x} {y} --ground")));
                assert_eq!(through, *bare.at(x, y), "pixel {x},{y}, through the models");
            }
        }
    }
    eprintln!("picks: {agree} of {picked} name what the sight frame shows, all {inner} inside");
}

fn a_lamp_moved_live_draws_as_one_added_there(viewer: &mut Viewer, at: &dyn Fn(&str) -> PathBuf) {
    let (id, _) = lamppost(viewer.placements());
    let lamp = viewer
        .placements()
        .get(id)
        .and_then(|p| p.filed.as_ref())
        .map(|f| f.model().to_owned())
        .expect("the lamppost's record");
    let [x, y] = super::LAMPPOST_XY;
    let added = viewer.ask(&format!("add {UNUSED_UNIQUE_ID} {lamp} {},{y}", x - 4.0));
    assert!(
        added.starts_with(&format!("ok placement {UNUSED_UNIQUE_ID}")),
        "{added}"
    );
    let moved = viewer.ask(&format!("move {UNUSED_UNIQUE_ID} --by 0,-3,0 --turn 30"));
    assert!(
        moved.starts_with(&format!("ok placement {UNUSED_UNIQUE_ID}")),
        "{moved}"
    );
    shoot(viewer, &at("4-moved-live"), "");
    viewer.ask(&format!("remove {UNUSED_UNIQUE_ID}"));
    shoot(viewer, &at("5-removed"), "");
    let facing = moved
        .split(" facing ")
        .nth(1)
        .and_then(|f| f.split_whitespace().next())
        .expect("a facing");
    let again = viewer.ask(&format!(
        "add {UNUSED_UNIQUE_ID} {lamp} {} --facing {facing}",
        placed_at(&moved)
    ));
    assert_eq!(
        placed_at(&again),
        placed_at(&moved),
        "added where it was moved to"
    );
    shoot(viewer, &at("6-added-there"), "");
    assert_eq!(
        pixels_differing(&at("4-moved-live"), &at("6-added-there")),
        0,
        "moved live, it draws as one added there"
    );
    assert!(pixels_differing(&at("4-moved-live"), &at("5-removed")) > 0);
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_viewer_picks_what_it_shows_shows_a_selection_and_a_ghost_alone_and_moves_a_lamp_live() {
    let Some(dir) = pictures() else {
        eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
        return;
    };
    let at = |name: &str| dir.join(format!("hands-{name}.png"));
    let mut viewer = Viewer::open(&format!("{} --no-glow --size {SIZE}", start()));
    shoot(&mut viewer, &at("1-plain"), "--seen");
    let index = viewer.app.world().resource::<SightIndex>();
    let shown = SightNames::read(&at("1-plain").with_extension("ids.png"), index);
    let every: Vec<String> = viewer
        .placements()
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    shoot(
        &mut viewer,
        &at("0-bare"),
        &format!("--leave-out {} --seen", every.join(",")),
    );
    let index = viewer.app.world().resource::<SightIndex>();
    let bare = SightNames::read(&at("0-bare").with_extension("ids.png"), index);
    picks_name_what_the_frame_shows(&mut viewer, &shown, &bare);

    let (lamp, _) = lamppost(viewer.placements());
    let selected = viewer.ask(&format!("select {lamp}"));
    shoot(&mut viewer, &at("2-selected"), "");
    let outside = differing_outside(&at("1-plain"), &at("2-selected"), &[on_frame(&selected)]);
    assert_eq!(
        outside, 0,
        "a selection changes nothing outside its box: {selected}"
    );
    assert!(
        pixels_differing(&at("1-plain"), &at("2-selected")) > 0,
        "and shows"
    );

    let (gx, gy) = shown.nearest_ground_to(shown.width / 2, shown.height() * 3 / 4);
    viewer.ask(&format!("pointer {gx} {gy}"));
    let ghost = viewer.ask(&format!("ghost {BARREL} --facing 40"));
    assert!(ghost.starts_with("ok ghost"), "{ghost}");
    let standing = viewer.app.world().resource::<Ghost>().filed.clone();
    viewer.ask(&format!("pick {} {}", shown.width / 4, shown.height() / 2));
    let after_a_pick = viewer.app.world().resource::<Ghost>().filed.clone();
    assert_eq!(
        after_a_pick, standing,
        "a pick leaves the ghost where it stood"
    );
    let [x, y, z] = standing.as_ref().map(Filed::position).expect("a ghost");
    viewer.ask(&format!("ghost {BARREL} {x},{y},{z} --facing 40"));
    viewer.ask(&format!(
        "pointer {} {}",
        shown.width / 4,
        shown.height() / 2
    ));
    let put = viewer.app.world().resource::<Ghost>().clone();
    assert_eq!(put.filed, standing, "a ghost put at a place stands there");
    assert_eq!(put.follows, None, "and follows no pointer");
    viewer.ask(&format!("pointer {gx} {gy}"));
    viewer.ask("select");
    shoot(&mut viewer, &at("3-ghost"), "");
    let outside = differing_outside(&at("1-plain"), &at("3-ghost"), &[on_frame(&ghost)]);
    assert_eq!(
        outside, 0,
        "a ghost changes nothing outside its box: {ghost}"
    );
    assert!(
        pixels_differing(&at("1-plain"), &at("3-ghost")) > 0,
        "and shows"
    );
    viewer.ask("ghost");

    a_lamp_moved_live_draws_as_one_added_there(&mut viewer, &at);
}

fn with_commands_held_off<T>(viewer: &mut Viewer, run: impl FnOnce(&mut App) -> T) -> T {
    let lines = viewer
        .app
        .world_mut()
        .remove_resource::<Lines>()
        .expect("the viewer's lines");
    let out = run(&mut viewer.app);
    viewer.app.insert_resource(lines);
    out
}

fn frame_times_ms(viewer: &mut Viewer, frames: usize) -> Vec<f64> {
    with_commands_held_off(viewer, |app| {
        (0..frames)
            .map(|_| {
                let t = std::time::Instant::now();
                app.update();
                t.elapsed().as_secs_f64() * 1e3
            })
            .collect()
    })
}

fn spread(mut times: Vec<f64>) -> String {
    times.sort_by(f64::total_cmp);
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    format!(
        "mean {mean:.3} ms, p50 {:.3}, p90 {:.3}, p99 {:.3}",
        at(0.5),
        at(0.9),
        at(0.99)
    )
}

fn cast_times_ms(viewer: &mut Viewer, casts: usize) -> Vec<f64> {
    use bevy::ecs::system::RunSystemOnce;
    use world::WorldCamera;
    use world::sight::Sight;

    let world = viewer.app.world_mut();
    world
        .run_system_once(
            move |sight: Sight<'_, '_>,
                  camera: Query<'_, '_, (&Camera, &GlobalTransform), With<WorldCamera>>| {
                let Ok((camera, eye)) = camera.single() else {
                    return Vec::new();
                };
                let middle = camera.logical_viewport_size().unwrap_or_default() / 2.0;
                (0..casts)
                    .filter_map(|k| {
                        let at = middle + Vec2::new(k as f32 % 40.0 - 20.0, 0.0);
                        let ray = camera.viewport_to_world(eye, at).ok()?;
                        let t = std::time::Instant::now();
                        let cast = sight.cast(ray.origin, ray.direction, world::FARCLIP);
                        let _ = cast.ground();
                        let _ = cast.first();
                        Some(t.elapsed().as_secs_f64() * 1e3)
                    })
                    .collect()
            },
        )
        .expect("the casts run")
}

#[test]
#[ignore = "draws on the GPU and takes a while; set WOW_DATA and CAIRN_PICTURES"]
fn the_frame_cost_of_a_selection_and_a_ghost() {
    const FRAMES: usize = 600;
    if pictures().is_none() {
        eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
        return;
    }
    let mut viewer = Viewer::open(&format!("{} --no-glow --size 1280x720", start()));
    viewer.ask("where");
    let (lamp, _) = lamppost(viewer.placements());
    for round in 1..=3 {
        viewer.ask("select");
        viewer.ask("ghost");
        eprintln!(
            "round {round}, bare: {}",
            spread(frame_times_ms(&mut viewer, FRAMES))
        );
        viewer.ask("pointer 640 600");
        viewer.ask(&format!("ghost {BARREL}"));
        viewer.ask(&format!("select {lamp}"));
        eprintln!(
            "round {round}, a selection and a ghost: {}",
            spread(frame_times_ms(&mut viewer, FRAMES))
        );
    }
    eprintln!("a pick's cast: {}", spread(cast_times_ms(&mut viewer, 200)));
}
