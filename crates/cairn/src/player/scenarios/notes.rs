use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::CharacterLook;
use world::{FOV_Y, PlacedModel, Placements, WorldCamera};

use super::INN_WALL;
use super::painter::{Painter, SIZE, STEP};
use super::pictures::FACING_A_GOLDSHIRE_LAMPPOST;
use crate::args::{self, Mode};
use crate::note::{Notes, Pointer};
use crate::player::camera::CameraControl;
use crate::player::state::{Player, TURN_RATE};

const WRITTEN_WITHIN: Duration = Duration::from_secs(60);
const SHOT_WITHIN: Duration = Duration::from_secs(300);
const INN_CHIMNEY_BEHIND_THE_LAMPPOST: UVec2 = UVec2::new(505, 100);
const RETINA: u32 = 2;
const MAGENTA: [u8; 3] = [255, 0, 255];

struct Written {
    dir: PathBuf,
    text: String,
}

impl Written {
    fn line(&self, key: &str) -> &str {
        self.text
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_else(|| panic!("the note has no {key} line"))
    }

    fn spot(&self) -> UVec2 {
        let pixel = self.line("spot: pixel ").split_whitespace().next();
        let xy = pixel.map(numbers).expect("a pixel");
        UVec2::new(xy[0] as u32, xy[1] as u32)
    }

    fn point_wow(&self) -> [f32; 3] {
        let at = self.line("at: ").split(", ").next().map(numbers);
        at.as_deref()
            .and_then(|at| <[f32; 3]>::try_from(at).ok())
            .unwrap_or_else(|| panic!("the at line: {at:?}"))
    }

    fn see_it(&self) -> &str {
        self.line("see it: cairn ")
    }

    fn camera(&self) -> NoteCamera {
        let flags = self.line("camera: ");
        let (eye, look) = flags
            .strip_prefix("--eye ")
            .and_then(|c| c.split_once(" --look "))
            .expect("the camera's flags");
        NoteCamera {
            eye: Vec3::from_slice(&numbers(eye)),
            look: Vec3::from_slice(&numbers(look)),
        }
    }

    fn see_it_from(&self, camera: NoteCamera) -> String {
        self.see_it()
            .replace(self.line("camera: "), &camera.flags())
    }
}

#[derive(Clone, Copy)]
struct NoteCamera {
    eye: Vec3,
    look: Vec3,
}

impl NoteCamera {
    fn flags(self) -> String {
        let (e, l) = (self.eye, self.look);
        format!(
            "--eye {},{},{} --look {},{},{}",
            e.x, e.y, e.z, l.x, l.y, l.z
        )
    }

    fn turned_west(self, radians: f32) -> Self {
        let look = self.eye + Quat::from_rotation_z(radians) * (self.look - self.eye);
        Self { look, ..self }
    }

    fn backed_off(self, yards: f32) -> Self {
        let back = (self.eye - self.look).normalize() * yards;
        Self {
            eye: self.eye + back,
            look: self.look + back,
        }
    }
}

fn numbers(list: &str) -> Vec<f32> {
    list.split(',')
        .map(|n| n.trim().parse().expect("a number"))
        .collect()
}

fn aim_at_pixel(p: &mut Painter, at: UVec2) {
    p.app.insert_resource(Pointer::Over(at));
}

fn window(p: &mut Painter) -> Mut<'_, Window> {
    let world = p.app.world_mut();
    world
        .query_filtered::<&mut Window, With<PrimaryWindow>>()
        .single_mut(world)
        .expect("the window")
}

fn move_the_pointer_to(p: &mut Painter, points: Vec2) {
    window(p).set_cursor_position(Some(points));
}

fn mistake_points_for_pixels(p: &mut Painter, points: Vec2) {
    window(p).set_physical_cursor_position(Some(points.as_dvec2()));
}

fn leave_note(p: &mut Painter) -> Written {
    let before = p.app.world().resource::<Notes>().written.len();
    let usual = (0..30).map(|_| p.timed_frame()).max().unwrap_or_default();
    let chord = [KeyCode::ControlLeft, KeyCode::ShiftLeft, KeyCode::KeyN];
    for key in chord {
        p.key(key, ButtonState::Pressed);
    }
    let pressed = p.timed_frame();
    let world_clock = p.clock().elapsed();
    for key in chord {
        p.key(key, ButtonState::Released);
    }
    p.clock().pause();
    let deadline = Instant::now() + WRITTEN_WITHIN;
    let (mut frames, mut writing) = (0, Duration::ZERO);
    while p.app.world().resource::<Notes>().written.len() == before {
        assert!(Instant::now() < deadline, "no note was written");
        writing = writing.max(p.timed_frame());
        frames += 1;
    }
    p.clock().unpause();
    let dir = p.app.world().resource::<Notes>().written[before].clone();
    let text = std::fs::read_to_string(dir.join("note.txt")).expect("the note's text");
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    eprintln!(
        "{}\n{text}taken {:.4} s into the world's clock; the chord's frame took {:.2} ms and \
         the {frames} frames until the note was written at most {:.2}; the 30 before at most \
         {:.2}; load {}",
        dir.display(),
        world_clock.as_secs_f64(),
        ms(pressed),
        ms(writing),
        ms(usual),
        server::load_average()
    );
    Written { dir, text }
}

fn drawn_by_the_painter(p: &mut Painter, wow: [f32; 3]) -> Vec2 {
    let world = p.app.world_mut();
    let (camera, placed) = world
        .query_filtered::<(&Camera, &GlobalTransform), With<WorldCamera>>()
        .single(world)
        .expect("the follow camera");
    camera
        .world_to_viewport(placed, wow_to_bevy(wow))
        .expect("in view")
}

struct Placed {
    unique_id: u32,
    file: String,
    foot_wow: Vec3,
}

fn nearest_placed(p: &Painter, xy: [f32; 2], file_holding: &str) -> Placed {
    let placements = p.app.world().resource::<Placements>();
    let mut near: Vec<(f32, Placed)> = placements
        .iter()
        .map(|(unique_id, placed)| {
            let (PlacedModel::Doodad { url } | PlacedModel::Building { url, .. }) = &placed.model;
            let foot_wow = Vec3::from(bevy_to_wow(placed.transform.translation));
            let away = (foot_wow.truncate() - Vec2::from(xy)).length();
            let file = url.trim_start_matches("mpq://").to_owned();
            let placed = Placed {
                unique_id,
                file,
                foot_wow,
            };
            (away, placed)
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.unique_id.cmp(&b.1.unique_id)));
    let Some((away, placed)) = near.into_iter().find(|n| n.1.file.contains(file_holding)) else {
        panic!("nothing holding {file_holding} near {xy:?}");
    };
    eprintln!(
        "{away:.2} yd away: unique id {}, {}",
        placed.unique_id, placed.file
    );
    placed
}

fn shoot_and_project(see_it: &str, out: &Path, wow: [f32; 3]) -> Vec2 {
    let argv = see_it.split_whitespace().map(str::to_owned);
    let mut args = args::parse(argv).expect("the note's flags parse");
    args.mode = Mode::Shot(out.to_path_buf());
    let (install, map, start) = crate::open(&args.map).expect("the map opens");
    let mut app = App::new();
    crate::client::assemble(&mut app, args, &install, map, start, std::convert::identity)
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

fn pixels_off(spot: UVec2, drawn: Vec2) -> f32 {
    (spot.as_vec2() + 0.5 - drawn).abs().max_element()
}

fn ring_centre(frame: &Image) -> Vec2 {
    let (width, data) = (frame.width(), frame.data.as_deref().expect("pixels"));
    let (mut sum, mut count) = (Vec2::ZERO, 0.0);
    for (i, px) in data.as_chunks::<4>().0.iter().enumerate() {
        if px[..3] == MAGENTA {
            let (x, y) = (i as u32 % width, i as u32 / width);
            sum += Vec2::new(x as f32, y as f32) + 0.5;
            count += 1.0;
        }
    }
    assert!(count > 0.0, "no ring in the frame");
    sum / count
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

struct Alignment {
    shift_px: i32,
    mean_channel_difference: f32,
}

fn best_sideways_alignment(
    a: &Image,
    b: &Image,
    cols: &[Range<u32>],
    rows: Range<u32>,
    reach_px: i32,
) -> Alignment {
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
    (-reach_px..=reach_px)
        .map(|shift_px| Alignment {
            shift_px,
            mean_channel_difference: apart(shift_px),
        })
        .min_by(|p, q| {
            let by_difference = p
                .mean_channel_difference
                .total_cmp(&q.mean_channel_difference);
            by_difference.then(p.shift_px.abs().cmp(&q.shift_px.abs()))
        })
        .expect("a shift")
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
    let lamp = nearest_placed(&p, stand.xy, "lamppost");
    let pole = lamp.foot_wow + Vec3::Z * 3.0;
    let aim = drawn_by_the_painter(&mut p, pole.to_array()).as_uvec2();
    aim_at_pixel(&mut p, aim);
    let note = leave_note(&mut p);
    let named = format!("doodad, unique id {}, {}", lamp.unique_id, lamp.file);
    assert_eq!(note.line("met: "), named);
    assert_eq!(note.spot(), aim);
    let ring = ring_centre(&load(&note.dir.join("frame.png")));
    assert_eq!(ring, aim.as_vec2() + 0.5, "the ring is round the spot");
    let feet = Vec3::from(bevy_to_wow(p.app.world().resource::<Player>().pos));
    let walk = note.line("walk there: cairn ").split_whitespace();
    let stands = args::parse(walk.map(str::to_owned)).expect("the window takes them");
    let stands = stands.pose.expect("a camera");
    let (above, aside) = (
        stands.target.z - feet.z,
        stands.target.truncate() - feet.truncate(),
    );
    eprintln!(
        "the window would stand the body {:.3} yd aside and {above:.3} above its feet",
        aside.length()
    );
    assert!(
        aside.length() < 0.5 && (0.0..2.0).contains(&above),
        "where the body stood"
    );

    let inn = nearest_placed(&p, stand.xy, ".wmo");
    aim_at_pixel(&mut p, INN_CHIMNEY_BEHIND_THE_LAMPPOST);
    let chimney = leave_note(&mut p);
    let building = format!(
        "building, unique id {}, {}, group ",
        inn.unique_id, inn.file
    );
    assert!(
        chimney.line("met: ").starts_with(&building),
        "{}",
        chimney.line("met: ")
    );

    p.orbit(std::f32::consts::PI, 8.0);
    p.tilt_up(0.9);
    p.wait(1.0);
    aim_at_pixel(&mut p, UVec2::new(SIZE.x - 60, 60));
    let sky = leave_note(&mut p);
    assert!(sky.line("met: ").starts_with("nothing within the far clip"));
    drop(p);

    let out = pictures().join("notes-lamppost-reopened.png");
    let drawn = shoot_and_project(note.see_it(), &out, note.point_wow());
    let off = pixels_off(note.spot(), drawn);
    eprintln!(
        "the lamppost's point, noted at pixel {}, is drawn again at {drawn}: {off:.3} px",
        note.spot()
    );
    assert!(off <= 1.0, "{off} px");
}

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
    aim_at_pixel(&mut p, UVec2::new(SIZE.x / 3, SIZE.y / 3));
    let note = leave_note(&mut p);
    p.key(KeyCode::KeyA, ButtonState::Released);
    drop(p);

    let noted = load(&note.dir.join("frame.png"));
    let cols = [130..520, 760..1150];
    let (rows, reach_px) = (60..300, 120);
    let out = pictures().join("notes-turning-reopened.png");
    let drawn = shoot_and_project(note.see_it(), &out, note.point_wow());
    let off = pixels_off(note.spot(), drawn);
    let same = best_sideways_alignment(&noted, &load(&out), &cols, rows.clone(), reach_px);
    eprintln!(
        "turning: noted at {}, drawn again at {drawn} ({off:.3} px); the frames match best \
         shifted {} px, {:.2} apart",
        note.spot(),
        same.shift_px,
        same.mean_channel_difference
    );
    assert!(off <= 1.0, "{off} px");
    assert_eq!(same.shift_px, 0, "the note's frame is its camera's");

    let a_frame_of_turn = TURN_RATE * STEP.as_secs_f32();
    let a_frame_of_turn_px = a_frame_of_turn / FOV_Y * SIZE.y as f32;
    let out = pictures().join("notes-turning-a-frame-on.png");
    let a_frame_on = note.see_it_from(note.camera().turned_west(a_frame_of_turn));
    shoot_and_project(&a_frame_on, &out, note.point_wow());
    let apart = best_sideways_alignment(&noted, &load(&out), &cols, rows, reach_px);
    eprintln!(
        "the camera a frame on ({a_frame_of_turn_px:.1} px of turn): the frames match best \
         shifted {} px, {:.2} apart",
        apart.shift_px, apart.mean_channel_difference
    );
    assert!(
        apart.shift_px.abs() as f32 > a_frame_of_turn_px / 2.0,
        "a frame apart shows: {} px",
        apart.shift_px
    );
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
    aim_at_pixel(&mut p, UVec2::new(SIZE.x / 5, SIZE.y / 3));
    let note = leave_note(&mut p);
    drop(p);

    let (spot, point) = (note.spot(), note.point_wow());
    let out = pictures().join("notes-wall-reopened.png");
    let drawn = shoot_and_project(note.see_it(), &out, point);
    let off = pixels_off(spot, drawn);
    eprintln!("noted at pixel {spot}, drawn again at {drawn}: {off:.3} px");
    assert!(off <= 1.0, "the camera as drawn: {off} px");

    let as_asked = note.see_it_from(note.camera().backed_off(pulled_in));
    let out = pictures().join("notes-wall-as-asked.png");
    let drawn = shoot_and_project(&as_asked, &out, point);
    let off = pixels_off(spot, drawn);
    eprintln!(
        "through the camera as asked, {pulled_in:.3} yd back, it is drawn at {drawn}: {off:.3} px"
    );
    assert!(off > 1.0, "the camera as asked: {off} px");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn on_a_retina_window_the_note_rings_names_and_reopens_the_lamppost_on_the_frames_own_pixel() {
    let stand = FACING_A_GOLDSHIRE_LAMPPOST;
    let Some(mut p) = Painter::new(stand.xy, stand.heading, CharacterLook::naked(1, 0)) else {
        return;
    };
    let frame_px = p.draw_for_a_window_at(RETINA);
    let window_points = frame_px / RETINA;
    p.orbit(0.0, 8.0);
    p.tilt_up(-0.1);
    p.wait(2.0);
    let lamp = nearest_placed(&p, stand.xy, "lamppost");
    let pole = lamp.foot_wow + Vec3::Z * 3.0;
    let points = drawn_by_the_painter(&mut p, pole.to_array());
    let pixel = (points * RETINA as f32).as_uvec2();
    move_the_pointer_to(&mut p, points);
    let note = leave_note(&mut p);
    let named = format!("doodad, unique id {}, {}", lamp.unique_id, lamp.file);
    let frame = load(&note.dir.join("frame.png"));
    let ring = ring_centre(&frame);
    eprintln!(
        "the pointer at {points} points is pixel {pixel} of a {frame_px} frame; noted at {}, \
         ringed round {ring}",
        note.spot()
    );
    assert_eq!(note.line("met: "), named);
    assert_eq!(note.spot(), pixel, "the frame's own pixel");
    assert_eq!(frame.size(), frame_px);
    assert_eq!(ring, pixel.as_vec2() + 0.5, "the ring is round the spot");
    let (frame_size, window_size) = (
        format!("{}x{}", frame_px.x, frame_px.y),
        format!("{}x{}", window_points.x, window_points.y),
    );
    assert!(
        note.see_it()
            .ends_with(&format!("--size {frame_size} --out view.png"))
    );
    let walk_there = note.line("walk there: cairn ");
    assert!(walk_there.ends_with(&format!("--size {window_size}")));

    mistake_points_for_pixels(&mut p, points);
    let mixed = leave_note(&mut p);
    let mixed_ring = ring_centre(&load(&mixed.dir.join("frame.png")));
    eprintln!(
        "the points taken for pixels: noted at {}, ringed round {mixed_ring}, met {}",
        mixed.spot(),
        mixed.line("met: ")
    );
    assert_ne!(mixed.spot(), pixel, "the points taken for pixels");
    assert_ne!(mixed.line("met: "), named, "the points taken for pixels");
    drop(p);

    let out = pictures().join("notes-retina-reopened.png");
    let drawn = shoot_and_project(note.see_it(), &out, note.point_wow());
    let off = pixels_off(note.spot(), drawn);
    eprintln!("reopened at {frame_size}, the lamppost's point is drawn at {drawn}: {off:.3} px");
    assert!(off <= 1.0, "{off} px");

    let at_window_size = note.see_it().replace(&frame_size, &window_size);
    let out = pictures().join("notes-retina-reopened-at-the-window-size.png");
    let drawn = shoot_and_project(&at_window_size, &out, note.point_wow());
    let off = pixels_off(note.spot(), drawn);
    eprintln!("reopened at {window_size}, it is drawn at {drawn}: {off:.3} px");
    assert!(
        off > 1.0,
        "the window's points taken for the frame's pixels: {off} px"
    );
}
