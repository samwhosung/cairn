//! Notes left in the window's headless harness: a note names what the pointer was over, and the
//! camera it records draws that spot on the same pixel again, where the camera the follow rig
//! asked for, before collision pulled it in, does not.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::input::ButtonState;
use bevy::prelude::*;
use world::coords::wow_to_bevy;
use world::unit::CharacterLook;
use world::{CurrentMap, Install, PlacedModel, Placements, WorldCamera};

use super::INN_WALL;
use super::pictures::{FACING_A_GOLDSHIRE_LAMPPOST, Painter, SIZE, STEP};
use crate::args::{self, Mode};
use crate::note::{Notes, Pointer};
use crate::player::camera::CameraControl;
use crate::player::state::Player;
use crate::player::state::TURN_RATE;

const WRITTEN_WITHIN: Duration = Duration::from_secs(60);
/// The inn's chimney, behind the lamppost from where the lamppost's check stands.
const CHIMNEY: UVec2 = UVec2::new(505, 100);
const SHOT_WITHIN: Duration = Duration::from_secs(300);

/// Points at `at` and presses Ctrl+Shift+N: the note's directory and text, once written, with
/// what the frames cost meanwhile.
fn leave_note(p: &mut Painter, at: UVec2) -> (PathBuf, String) {
    p.app.insert_resource(Pointer(Some(at)));
    let before = p.app.world().resource::<Notes>().written.len();
    let usual = (0..30).map(|_| p.timed_frame()).max().unwrap_or_default();
    let chord = [KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::KeyN];
    for key in chord {
        p.key(key, ButtonState::Pressed);
    }
    let pressed = p.timed_frame();
    for key in chord {
        p.key(key, ButtonState::Released);
    }
    let deadline = Instant::now() + WRITTEN_WITHIN;
    let (mut frames, mut writing) = (0, Duration::ZERO);
    while p.app.world().resource::<Notes>().written.len() == before {
        assert!(Instant::now() < deadline, "no note was written");
        writing = writing.max(p.timed_frame());
        frames += 1;
    }
    let dir = p.app.world().resource::<Notes>().written[before].clone();
    let text = std::fs::read_to_string(dir.join("note.txt")).expect("the note's text");
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    eprintln!(
        "{}\n{text}the chord's frame took {:.2} ms and the {frames} frames until the note was \
         written at most {:.2}; the 30 before at most {:.2}; load {}",
        dir.display(),
        ms(pressed),
        ms(writing),
        ms(usual),
        server::load_average()
    );
    (dir, text)
}

fn line<'a>(text: &'a str, key: &str) -> &'a str {
    text.lines()
        .find_map(|l| l.strip_prefix(key))
        .unwrap_or_else(|| panic!("the note has no {key} line"))
}

fn numbers(list: &str) -> Vec<f32> {
    list.split(',')
        .map(|n| n.trim().parse().expect("a number"))
        .collect()
}

/// The spot's pixel, and the world point the ray under it met.
fn spot_and_point(text: &str) -> (UVec2, [f32; 3]) {
    let pixel = line(text, "spot: pixel ")
        .split_whitespace()
        .next()
        .map(numbers)
        .expect("a pixel");
    let at = line(text, "at: ").split(", ").next().map(numbers);
    let Some([x, y, z]) = at.as_deref().and_then(|at| <[f32; 3]>::try_from(at).ok()) else {
        panic!("the at line: {at:?}");
    };
    (UVec2::new(pixel[0] as u32, pixel[1] as u32), [x, y, z])
}

/// Where the painter's own camera draws a WoW point.
fn drawn_at(p: &mut Painter, wow: [f32; 3]) -> Vec2 {
    let world = p.app.world_mut();
    let (camera, placed) = world
        .query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>()
        .single(world)
        .expect("the follow camera");
    camera
        .world_to_viewport(placed, wow_to_bevy(wow))
        .expect("in view")
}

/// The placement whose file holds `named` nearest `xy`: its unique id, file and where it stands.
fn nearest_placed(p: &Painter, xy: [f32; 2], named: &str) -> (u32, String, Vec3) {
    let placements = p.app.world().resource::<Placements>();
    let at = |t: &Transform| Vec3::from(world::coords::bevy_to_wow(t.translation));
    let mut near: Vec<(f32, u32, String, Vec3)> = placements
        .iter()
        .map(|(id, placed)| {
            let (PlacedModel::Doodad { url } | PlacedModel::Building { url, .. }) = &placed.model;
            let foot = at(&placed.transform);
            let away = (foot.truncate() - Vec2::from(xy)).length();
            (away, id, url.trim_start_matches("mpq://").to_owned(), foot)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let Some((away, id, file, foot)) = near.into_iter().find(|n| n.2.contains(named)) else {
        panic!("nothing named {named} near {xy:?}");
    };
    eprintln!("{away:.2} yd away: unique id {id}, {file}");
    (id, file, foot)
}

/// Runs `cairn shot` in this process on a note's `see it` flags, its frame written to `out`:
/// where its camera draws the WoW point.
fn reopen(see_it: &str, out: &Path, wow: [f32; 3]) -> Vec2 {
    let argv = see_it.split_whitespace().map(str::to_owned);
    let mut args = args::parse(argv).expect("the note's flags parse");
    args.mode = Mode::Shot(out.to_path_buf());
    let data = std::env::var_os("WOW_DATA").expect("the install");
    let install = Install::open(Path::new(&data)).expect("the install opens");
    let map = CurrentMap::find(&install.0, &args.map).expect("the map");
    let mut app = App::new();
    crate::client::assemble(&mut app, args, &install, map, std::convert::identity)
        .expect("the shot assembles");
    app.finish();
    app.cleanup();
    let deadline = Instant::now() + SHOT_WITHIN;
    while app.should_exit().is_none() {
        assert!(Instant::now() < deadline, "the shot never came");
        app.update();
    }
    assert_eq!(app.should_exit(), Some(AppExit::Success));
    let world = app.world_mut();
    let (camera, placed) = world
        .query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>()
        .single(world)
        .expect("the shot's camera");
    camera
        .world_to_viewport(placed, wow_to_bevy(wow))
        .expect("in view")
}

fn pictures() -> PathBuf {
    PathBuf::from(std::env::var_os("CAIRN_PICTURES").expect("CAIRN_PICTURES"))
}

/// How far the spot's pixel centre is from where a camera draws its point, in pixels.
fn miss(spot: UVec2, drawn: Vec2) -> f32 {
    (spot.as_vec2() + 0.5 - drawn).abs().max_element()
}

fn load(path: &Path) -> Image {
    let bytes = std::fs::read(path).expect("the frame");
    Image::from_buffer(
        &bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::MAIN_WORLD,
    )
    .expect("a PNG")
}

/// The sideways shift of `b`, within `reach` pixels, that best matches `a` over the rows and
/// columns given, and the mean channel difference there, out of 255.
fn best_shift(
    a: &Image,
    b: &Image,
    cols: &[Range<u32>],
    rows: Range<u32>,
    reach: i32,
) -> (i32, f32) {
    let (a, b, width) = (a.data.as_deref(), b.data.as_deref(), a.width());
    let (Some(a), Some(b)) = (a, b) else {
        panic!("frames without pixels");
    };
    let rgb = |data: &[u8], x: u32, y: u32| {
        let at = ((y * width + x) * 4) as usize;
        [data[at], data[at + 1], data[at + 2]]
    };
    let apart = |dx: i32| {
        let (mut sum, mut count) = (0u64, 0u64);
        for y in rows.clone() {
            for x in cols.iter().flat_map(Clone::clone) {
                let shifted = x.saturating_add_signed(dx).min(width - 1);
                let (p, q) = (rgb(a, x, y), rgb(b, shifted, y));
                sum += p
                    .iter()
                    .zip(q)
                    .map(|(p, q)| u64::from(p.abs_diff(q)))
                    .sum::<u64>();
                count += 3;
            }
        }
        sum as f32 / count as f32
    };
    (-reach..=reach)
        .map(|dx| (dx, apart(dx)))
        .min_by(|p, q| p.1.total_cmp(&q.1).then(p.0.abs().cmp(&q.0.abs())))
        .expect("a shift")
}

/// The note's `see it` flags with its camera turned about the eye by `yaw` radians, west of north.
fn turned(text: &str, yaw: f32) -> String {
    let camera = line(text, "camera: ");
    let (eye, look) = camera
        .strip_prefix("--eye ")
        .and_then(|c| c.split_once(" --look "))
        .expect("the camera's flags");
    let (eye, look) = (
        Vec3::from_slice(&numbers(eye)),
        Vec3::from_slice(&numbers(look)),
    );
    let look = eye + Quat::from_rotation_z(yaw) * (look - eye);
    let flags = format!(
        "--eye {},{},{} --look {},{},{}",
        eye.x, eye.y, eye.z, look.x, look.y, look.z
    );
    line(text, "see it: cairn ").replace(camera, &flags)
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_note_names_the_lamppost_pointed_at_and_its_camera_draws_it_on_the_same_pixel() {
    let stand = FACING_A_GOLDSHIRE_LAMPPOST;
    let Some(mut p) = Painter::new(stand.xy, stand.heading, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.orbit(0.0, 8.0);
    p.tilt_up(-0.1);
    p.wait(2.0);
    let (lamp, file, foot) = nearest_placed(&p, stand.xy, "lamppost");
    let pole = foot + Vec3::Z * 3.0;
    let aim = drawn_at(&mut p, pole.to_array()).as_uvec2();
    let (_, text) = leave_note(&mut p, aim);
    assert_eq!(
        line(&text, "met: "),
        format!("doodad, unique id {lamp}, {file}")
    );
    let (spot, point) = spot_and_point(&text);
    assert_eq!(spot, aim);
    let feet = Vec3::from(world::coords::bevy_to_wow(
        p.app.world().resource::<Player>().pos,
    ));
    let walk = line(&text, "walk there: cairn ").split_whitespace();
    let stands = args::parse(walk.map(str::to_owned)).expect("the window takes them");
    let (above, aside) = (
        stands.pose.target.z - feet.z,
        stands.pose.target.truncate() - feet.truncate(),
    );
    eprintln!(
        "the window would stand the body {:.3} yd aside and {above:.3} above its feet",
        aside.length()
    );
    assert!(
        aside.length() < 0.5 && (0.0..2.0).contains(&above),
        "where the body stood"
    );

    let (inn, file, _) = nearest_placed(&p, stand.xy, ".wmo");
    let (_, text) = leave_note(&mut p, CHIMNEY);
    let met = line(&text, "met: ");
    assert!(
        met.starts_with(&format!("building, unique id {inn}, {file}, group ")),
        "{met}"
    );

    p.orbit(std::f32::consts::PI, 8.0);
    p.tilt_up(0.9);
    p.wait(1.0);
    let (_, sky) = leave_note(&mut p, UVec2::new(SIZE.x - 60, 60));
    assert!(line(&sky, "met: ").starts_with("nothing within the far clip"));
    drop(p);

    let out = pictures().join("notes-lamppost-reopened.png");
    let drawn = reopen(line(&text, "see it: cairn "), &out, point);
    let off = miss(spot, drawn);
    eprintln!(
        "the lamppost's point, noted at pixel {spot}, is drawn again at {drawn}: {off:.3} px"
    );
    assert!(off <= 1.0, "{off} px");
}

/// Turning at the client's rate, the camera moves 53 px a frame: a note whose frame and camera
/// were a frame apart would find its frame shifted that far from the shot of its camera.
#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_note_taken_while_the_camera_turns_keeps_the_frame_its_camera_drew() {
    let stand = FACING_A_GOLDSHIRE_LAMPPOST;
    let Some(mut p) = Painter::new(stand.xy, stand.heading, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.orbit(0.0, 8.0);
    p.tilt_up(-0.1);
    p.wait(2.0);
    p.key(KeyCode::KeyA, ButtonState::Pressed);
    p.wait(0.25);
    let (dir, text) = leave_note(&mut p, UVec2::new(SIZE.x / 3, SIZE.y / 3));
    p.key(KeyCode::KeyA, ButtonState::Released);
    let (spot, point) = spot_and_point(&text);
    drop(p);

    let noted = load(&dir.join("frame.png"));
    let cols = [130..520, 760..1150];
    let (rows, reach) = (60..300, 120);
    let out = pictures().join("notes-turning-reopened.png");
    let drawn = reopen(line(&text, "see it: cairn "), &out, point);
    let off = miss(spot, drawn);
    let (shift, apart) = best_shift(&noted, &load(&out), &cols, rows.clone(), reach);
    eprintln!(
        "turning: noted at {spot}, drawn again at {drawn} ({off:.3} px); the frames match best \
         shifted {shift} px, {apart:.2} apart"
    );
    assert!(off <= 1.0, "{off} px");
    assert_eq!(shift, 0, "the note's frame is its camera's");

    let a_frame_on = TURN_RATE * STEP.as_secs_f32();
    let out = pictures().join("notes-turning-a-frame-on.png");
    reopen(&turned(&text, a_frame_on), &out, point);
    let (shift, apart) = best_shift(&noted, &load(&out), &cols, rows, reach);
    eprintln!("the camera a frame on: the frames match best shifted {shift} px, {apart:.2} apart");
    assert!(shift.abs() > 40, "a frame apart shows: {shift} px");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_camera_pulled_in_by_a_wall_is_noted_as_drawn_and_the_one_asked_for_misses() {
    let ([nx, ny], c) = INN_WALL;
    let y = 23.0;
    let wall = [(c - ny * y) / nx, y];
    let stand = [wall[0] + 2.0 * nx, wall[1] + 2.0 * ny];
    let away_from_the_wall = ny.atan2(nx).to_degrees();
    let Some(mut p) = Painter::new(stand, away_from_the_wall, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.wait(2.0);
    let control = p.app.world().resource::<CameraControl>();
    let pulled_in = control.distance - control.boom_length;
    eprintln!(
        "the boom asked for {:.3} yd and was pulled in to {:.3}",
        control.distance, control.boom_length
    );
    assert!(pulled_in > 5.0, "the wall pulls the camera in: {pulled_in}");
    let aim = UVec2::new(SIZE.x / 5, SIZE.y / 3);
    let (_, text) = leave_note(&mut p, aim);
    let (spot, point) = spot_and_point(&text);
    drop(p);

    let see_it = line(&text, "see it: cairn ");
    let drawn = reopen(see_it, &pictures().join("notes-wall-reopened.png"), point);
    let off = miss(spot, drawn);
    eprintln!("noted at pixel {spot}, drawn again at {drawn}: {off:.3} px");
    assert!(off <= 1.0, "the camera as drawn: {off} px");

    let camera = line(&text, "camera: ");
    let (eye, look) = camera
        .strip_prefix("--eye ")
        .and_then(|c| c.split_once(" --look "))
        .expect("the camera's flags");
    let (eye, look) = (
        Vec3::from_slice(&numbers(eye)),
        Vec3::from_slice(&numbers(look)),
    );
    let asked = eye - (look - eye).normalize() * pulled_in;
    let as_asked = see_it.replace(
        camera,
        &format!(
            "--eye {},{},{} --look {},{},{}",
            asked.x,
            asked.y,
            asked.z,
            asked.x + look.x - eye.x,
            asked.y + look.y - eye.y,
            asked.z + look.z - eye.z
        ),
    );
    let out = pictures().join("notes-wall-as-asked.png");
    let drawn = reopen(&as_asked, &out, point);
    let off = miss(spot, drawn);
    eprintln!(
        "through the camera as asked, {pulled_in:.3} yd back, it is drawn at {drawn}: {off:.3} px"
    );
    assert!(off > 1.0, "the camera as asked: {off} px");
}
