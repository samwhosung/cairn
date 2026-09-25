//! Notes about a spot in the window. Ctrl+Shift+N keeps the frame the window drew with the spot
//! ringed, the camera it was drawn from and what the ray under the pointer met, in a directory
//! named by its time.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::camera::RenderTarget;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy::window::{CursorOptions, PrimaryWindow};
use world::coords::bevy_to_wow;
use world::sight::{Ray, Seen, Sight, Sighting};
use world::{CurrentMap, FARCLIP, FullScreenGlow, NEARCLIP, TimeOfDay, WorldCamera};

use crate::player::{Mode, Player};
use crate::shot::write_png;

/// How far out the camera's look point sits: near enough to stand the body on, far enough that
/// its coordinates, written to the thousandth of a yard, still aim within a tenth of a pixel.
const LOOK_NEAR: f32 = 5.0;
const LOOK_FAR: f32 = 20.0;
const FRAME: &str = "frame.png";
const TEXT: &str = "note.txt";
const MAGENTA: Color = Color::srgb(1.0, 0.0, 1.0);

/// Leaves a note in `dir` when Ctrl+Shift+N is pressed; without one, the chord only says so.
pub struct NotePlugin {
    pub dir: Option<PathBuf>,
}

impl Plugin for NotePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Notes {
            dir: self.dir.clone(),
            written: Vec::new(),
        })
        .init_resource::<Pointer>()
        .add_systems(Update, finish_notes)
        .add_systems(Last, (follow_cursor, take_note).chain());
    }
}

/// The pixel of the frame the pointer is over, or `None` for the middle.
#[derive(Resource, Default)]
pub struct Pointer(pub Option<UVec2>);

#[derive(Resource)]
pub struct Notes {
    dir: Option<PathBuf>,
    /// Every note written since the window opened, oldest first.
    pub written: Vec<PathBuf>,
}

#[derive(Component)]
struct Writing(Task<Result<PathBuf, String>>);

fn follow_cursor(
    window: Query<'_, '_, (&Window, &CursorOptions), With<PrimaryWindow>>,
    mut pointer: ResMut<'_, Pointer>,
) {
    if let Ok((window, cursor)) = window.single() {
        pointer.0 = window
            .physical_cursor_position()
            .filter(|_| cursor.visible)
            .map(|at| at.as_uvec2());
    }
}

#[derive(SystemParam)]
struct Scene<'w, 's> {
    camera: Query<
        'w,
        's,
        (
            &'static Camera,
            &'static GlobalTransform,
            &'static RenderTarget,
        ),
        With<WorldCamera>,
    >,
    sight: Sight<'w, 's>,
    map: Res<'w, CurrentMap>,
    time: Res<'w, TimeOfDay>,
    glow: Res<'w, FullScreenGlow>,
    player: Res<'w, Player>,
    mode: Res<'w, Mode>,
}

/// Runs last, so the camera is the one this frame is drawn from, and asks for this frame.
fn take_note(
    keys: Res<'_, ButtonInput<KeyCode>>,
    pointer: Res<'_, Pointer>,
    notes: Res<'_, Notes>,
    scene: Scene<'_, '_>,
    mut commands: Commands<'_, '_>,
) {
    let chord = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
        && keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !(chord && keys.just_pressed(KeyCode::KeyN)) {
        return;
    }
    let Some(root) = notes.dir.clone() else {
        warn!("no note: there is no data directory for notes; give --notes DIR");
        return;
    };
    let (Ok((_, _, target)), Some(taken)) = (scene.camera.single(), gather(&scene, pointer.0))
    else {
        warn!("no note: the camera has no frame yet");
        return;
    };
    let mut note = Some(Note {
        root,
        name: dir_name(taken.facts.taken),
        taken,
    });
    let write = move |captured: On<'_, '_, ScreenshotCaptured>, mut commands: Commands<'_, '_>| {
        if let Some(note) = note.take() {
            let frame = captured.image.clone();
            let task = AsyncComputeTaskPool::get().spawn(async move { note.write(frame) });
            commands.spawn(Writing(task));
        }
    };
    commands.spawn(Screenshot(target.clone())).observe(write);
}

fn finish_notes(
    mut commands: Commands<'_, '_>,
    mut writing: Query<'_, '_, (Entity, &mut Writing)>,
    mut notes: ResMut<'_, Notes>,
) {
    for (entity, mut task) in &mut writing {
        let Some(done) = block_on(poll_once(&mut task.0)) else {
            continue;
        };
        commands.entity(entity).despawn();
        match done {
            Ok(dir) => {
                info!("a note in {}", dir.display());
                notes.written.push(dir);
            }
            Err(e) => warn!("no note: {e}"),
        }
    }
}

/// A note as its chord takes it, the rays cast and still to be followed off the main thread.
struct Taken {
    facts: Facts,
    /// Bevy's axes.
    eye: Vec3,
    forward: Dir3,
    feet: Vec3,
    /// Through the spot, and along the camera's line of sight.
    under: Ray,
    ahead: Ray,
}

impl Taken {
    fn text(self) -> String {
        let met = self.under.first().map(|hit| Met {
            from_eye: hit.point.distance(self.eye),
            hit,
        });
        let blocked = self.ahead.first().map(|hit| hit.distance);
        let look = look_point(self.eye, self.forward, self.feet, blocked);
        self.facts.text(bevy_to_wow(look), met.as_ref())
    }
}

fn gather(scene: &Scene<'_, '_>, pointer: Option<UVec2>) -> Option<Taken> {
    let (camera, placed, _) = scene.camera.single().ok()?;
    let frame = camera.physical_target_size()?;
    let window = camera.logical_target_size()?.round().as_uvec2();
    let spot = pointer
        .unwrap_or(frame / 2)
        .min(frame.saturating_sub(UVec2::ONE));
    let scale = camera.target_scaling_factor()?;
    let ray = camera
        .viewport_to_world(placed, (spot.as_vec2() + 0.5) / scale)
        .ok()?;
    let (eye, forward, feet) = (placed.translation(), placed.forward(), scene.player.pos);
    let depth_left = (FARCLIP - NEARCLIP) / ray.direction.dot(*forward).max(f32::EPSILON);
    let taken = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let facts = Facts {
        taken,
        map: scene.map.directory.clone(),
        map_id: scene.map.id,
        minute: scene.time.minute,
        glow: scene.glow.0,
        frame,
        window,
        eye: bevy_to_wow(eye),
        flying: *scene.mode == Mode::Fly,
        feet: bevy_to_wow(feet),
        facing: scene.player.face_yaw,
        spot,
    };
    Some(Taken {
        facts,
        eye,
        forward,
        feet,
        under: scene.sight.cast(ray.origin, ray.direction, depth_left),
        ahead: scene.sight.cast(eye, forward, FARCLIP),
    })
}

/// A point on the camera's line of sight for its flags to look at, where the line passes nearest
/// the body's feet, clamped out from the eye and short of the first thing the line meets:
/// `cairn` stands the body there.
fn look_point(eye: Vec3, forward: Dir3, feet: Vec3, blocked: Option<f32>) -> Vec3 {
    let mut out = (feet - eye).dot(*forward).clamp(LOOK_NEAR, LOOK_FAR);
    if let Some(blocked) = blocked {
        out = out.min((blocked - 1.0).max(1.0));
    }
    eye + *forward * out
}

struct Met {
    hit: Sighting,
    from_eye: f32,
}

/// What a note says, taken when its chord is pressed. Positions are WoW's.
struct Facts {
    taken: Duration,
    map: String,
    map_id: u32,
    minute: u32,
    glow: bool,
    /// The frame's size in its pixels, and the window's in the screen's points.
    frame: UVec2,
    window: UVec2,
    eye: [f32; 3],
    flying: bool,
    feet: [f32; 3],
    /// Radians from north toward west.
    facing: f32,
    spot: UVec2,
}

impl Facts {
    /// The note's text, with the camera looking at `look` and the ray under the spot meeting
    /// `met`.
    fn text(&self, look: [f32; 3], met: Option<&Met>) -> String {
        let (hour, minute) = (self.minute / 60, self.minute % 60);
        let glow = if self.glow { "" } else { " --no-glow" };
        let camera = format!("--eye {} --look {}", xyz(self.eye), xyz(look));
        let view = format!(
            "--map {} --time {hour:02}:{minute:02}{glow} {camera}",
            self.map
        );
        let facing = self.facing.to_degrees().rem_euclid(360.0);
        let (spot, frame, window) = (self.spot, self.frame, self.window);
        let mut lines = vec![
            format!("note: {} UTC", utc(self.taken)),
            format!(
                "map: {} ({}) at {hour:02}:{minute:02}",
                self.map, self.map_id
            ),
            format!("frame: {FRAME}, {}x{}, the spot ringed", frame.x, frame.y),
            format!("camera: {camera}"),
            format!(
                "player: {}, feet at {}, facing {facing:.1} degrees from north toward west",
                if self.flying { "flying" } else { "walking" },
                xyz(self.feet)
            ),
            format!("spot: pixel {},{} from the top left", spot.x, spot.y),
        ];
        match met {
            None => lines.push(format!(
                "met: nothing within the far clip, {FARCLIP} yd of view depth"
            )),
            Some(Met { hit, from_eye }) => {
                let adt = self.adt(hit.tile);
                lines.push(format!("met: {}", named(&hit.seen, &adt)));
                lines.push(format!(
                    "at: {}, {from_eye:.2} yd from the eye, over {adt}",
                    xyz(bevy_to_wow(hit.point))
                ));
            }
        }
        lines.push(format!(
            "see it: cairn shot {view} --size {}x{} --out view.png",
            frame.x, frame.y
        ));
        lines.push(format!(
            "walk there: cairn {view} --size {}x{}",
            window.x, window.y
        ));
        lines.join("\n") + "\n"
    }

    fn adt(&self, (x, y): (u32, u32)) -> String {
        let map = self.map.to_ascii_lowercase();
        format!("world/maps/{map}/{map}_{x}_{y}.adt")
    }
}

fn named(seen: &Seen, adt: &str) -> String {
    match seen {
        Seen::Terrain { chunk: (x, y) } => {
            format!("terrain, chunk {x},{y} (MCNK {}) of {adt}", y * 16 + x)
        }
        Seen::Doodad { file, unique_id } => format!("doodad, unique id {unique_id}, {file}"),
        Seen::Building {
            file,
            unique_id,
            group,
        } => format!("building, unique id {unique_id}, {file}, group {group}"),
        Seen::Prop {
            file,
            building,
            unique_id,
            doodad,
        } => format!("{file}, doodad {doodad} of building unique id {unique_id}, {building}"),
    }
}

/// Written as the flags take them, each to the precision that reads back the same.
fn xyz([x, y, z]: [f32; 3]) -> String {
    format!("{x},{y},{z}")
}

struct Note {
    root: PathBuf,
    name: String,
    taken: Taken,
}

impl Note {
    fn write(self, mut frame: Image) -> Result<PathBuf, String> {
        let dir = make_dir(&self.root, &self.name)?;
        ring(&mut frame, self.taken.facts.spot);
        write_png(&frame, &dir.join(FRAME))?;
        let text = dir.join(TEXT);
        std::fs::write(&text, self.taken.text())
            .map_err(|e| format!("writing {}: {e}", text.display()))?;
        Ok(dir)
    }
}

/// A note's own directory in `root`: `name`, or `name` and a count when two notes share a second.
fn make_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
    let fail = |at: &Path, e: std::io::Error| format!("making {}: {e}", at.display());
    std::fs::create_dir_all(root).map_err(|e| fail(root, e))?;
    let mut dir = root.join(name);
    for n in 2.. {
        match std::fs::create_dir(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                dir = root.join(format!("{name}-{n}"));
            }
            Err(e) => return Err(fail(&dir, e)),
            Ok(()) => return Ok(dir),
        }
    }
    Err(fail(&dir, std::io::ErrorKind::AlreadyExists.into()))
}

/// A ring round the spot, magenta edged in black, that leaves the spot's own pixel as drawn.
fn ring(frame: &mut Image, spot: UVec2) {
    let size = frame.size();
    let r = (size.y as f32 / 60.0).max(8.0);
    let reach = (r + 2.0).ceil() as i32;
    let within =
        |d2: f32, band: f32| ((r - band) * (r - band)..=(r + band) * (r + band)).contains(&d2);
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let d2 = (dx * dx + dy * dy) as f32;
            let color = if within(d2, 1.0) {
                MAGENTA
            } else if within(d2, 2.0) {
                Color::BLACK
            } else {
                continue;
            };
            let (x, y) = (spot.x as i32 + dx, spot.y as i32 + dy);
            if let (Ok(x), Ok(y)) = (u32::try_from(x), u32::try_from(y))
                && x < size.x
                && y < size.y
            {
                frame.set_color_at(x, y, color).ok();
            }
        }
    }
}

/// `2026-09-25T17-12-09Z`: sorted by name, the notes are sorted by time.
fn dir_name(since_epoch: Duration) -> String {
    let [y, mo, d, h, mi, s] = civil(since_epoch);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}-{mi:02}-{s:02}Z")
}

fn utc(since_epoch: Duration) -> String {
    let [y, mo, d, h, mi, s] = civil(since_epoch);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// Year, month, day, hour, minute and second in UTC, by the proleptic Gregorian calendar.
fn civil(since_epoch: Duration) -> [u64; 6] {
    let secs = since_epoch.as_secs();
    let (days, of_day) = (secs / 86_400, secs % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let of_era = z % 146_097;
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    [
        year,
        month,
        day,
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
    ]
}

#[cfg(test)]
mod tests;
