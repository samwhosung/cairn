use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bevy::camera::RenderTarget;
use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, poll_once};
use bevy::transform::TransformSystems;
use bevy::window::{CursorOptions, PrimaryWindow};
use world::coords::bevy_to_wow;
use world::sight::{Cast, Seen, Sight, Sighting};
use world::{CurrentMap, FARCLIP, FullScreenGlow, NEARCLIP, TimeOfDay, WorldCamera};

use crate::player::{Mode, Player};
use crate::shot::write_png;

const AIMS_TRUE_FROM: f32 = 5.0;
const STANDS_WITHIN: f32 = 20.0;
const FRAME: &str = "frame.png";
const TEXT: &str = "note.txt";
const MAGENTA: Color = Color::srgb(1.0, 0.0, 1.0);

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
        .add_systems(
            PostUpdate,
            (follow_cursor, take_note)
                .chain()
                .after(TransformSystems::Propagate)
                .after(VisibilitySystems::CheckVisibility),
        );
    }
}

#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    #[default]
    Away,
    Over(UVec2),
}

#[derive(Resource)]
pub struct Notes {
    dir: Option<PathBuf>,
    pub written: Vec<PathBuf>,
}

#[derive(Component)]
struct Writing(Task<Result<PathBuf, String>>);

fn follow_cursor(
    window: Query<'_, '_, (&Window, &CursorOptions), With<PrimaryWindow>>,
    mut pointer: ResMut<'_, Pointer>,
) {
    if let Ok((window, cursor)) = window.single() {
        *pointer = match window.physical_cursor_position() {
            Some(at) if cursor.visible => Pointer::Over(at.as_uvec2()),
            _ => Pointer::Away,
        };
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
    let (Ok((_, _, target)), Some(pending)) = (scene.camera.single(), gather(&scene, *pointer))
    else {
        warn!("no note: the camera has no frame yet");
        return;
    };
    let mut note = Some(Note {
        root,
        name: UtcTime::at(pending.facts.taken).dir_name(),
        pending,
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

struct PendingNote {
    facts: Facts,
    camera: GlobalTransform,
    feet: Vec3,
    through_spot: Cast,
    line_of_sight: Cast,
}

impl PendingNote {
    fn text(self) -> String {
        let eye = self.camera.translation();
        let met = self.through_spot.first().map(|hit| Met {
            from_eye: hit.point.distance(eye),
            hit,
        });
        let blocked = self.line_of_sight.first().map(|hit| hit.distance);
        let look = look_nearest_the_feet(eye, self.camera.forward(), self.feet, blocked);
        self.facts.text(bevy_to_wow(look), met.as_ref())
    }
}

fn gather(scene: &Scene<'_, '_>, pointer: Pointer) -> Option<PendingNote> {
    let (camera, placed, _) = scene.camera.single().ok()?;
    let frame_px = camera.physical_target_size()?;
    let window_points = camera.logical_target_size()?.round().as_uvec2();
    let spot = match pointer {
        Pointer::Over(at) => at.min(frame_px.saturating_sub(UVec2::ONE)),
        Pointer::Away => frame_px / 2,
    };
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
        frame_px,
        window_points,
        eye_wow: bevy_to_wow(eye),
        flying: *scene.mode == Mode::Fly,
        feet_wow: bevy_to_wow(feet),
        heading: scene.player.face_yaw,
        spot,
    };
    Some(PendingNote {
        facts,
        camera: *placed,
        feet,
        through_spot: scene.sight.cast(ray.origin, ray.direction, depth_left),
        line_of_sight: scene.sight.cast(eye, forward, FARCLIP),
    })
}

fn look_nearest_the_feet(eye: Vec3, forward: Dir3, feet: Vec3, blocked: Option<f32>) -> Vec3 {
    let mut out = (feet - eye)
        .dot(*forward)
        .clamp(AIMS_TRUE_FROM, STANDS_WITHIN);
    if let Some(blocked) = blocked {
        out = out.min(blocked - 1.0).max(AIMS_TRUE_FROM);
    }
    eye + *forward * out
}

struct Met {
    hit: Sighting,
    from_eye: f32,
}

struct Facts {
    taken: Duration,
    map: String,
    map_id: u32,
    minute: u32,
    glow: bool,
    frame_px: UVec2,
    window_points: UVec2,
    eye_wow: [f32; 3],
    flying: bool,
    feet_wow: [f32; 3],
    heading: f32,
    spot: UVec2,
}

impl Facts {
    fn text(&self, look_wow: [f32; 3], met: Option<&Met>) -> String {
        let (hour, minute) = (self.minute / 60, self.minute % 60);
        let glow = if self.glow { "" } else { " --no-glow" };
        let camera = format!(
            "--eye {} --look {}",
            flag_xyz(self.eye_wow),
            flag_xyz(look_wow)
        );
        let view = format!(
            "--map {} --time {hour:02}:{minute:02}{glow} {camera}",
            self.map
        );
        let heading = self.heading.to_degrees().rem_euclid(360.0);
        let (spot, frame, window) = (self.spot, self.frame_px, self.window_points);
        let mut lines = vec![
            format!("note: {} UTC", UtcTime::at(self.taken).stamp()),
            format!(
                "map: {} ({}) at {hour:02}:{minute:02}",
                self.map, self.map_id
            ),
            format!("frame: {FRAME}, {}x{}, the spot ringed", frame.x, frame.y),
            format!("camera: {camera}"),
            format!(
                "player: {}, feet at {}, facing {heading:.1} degrees from north toward west",
                if self.flying { "flying" } else { "walking" },
                flag_xyz(self.feet_wow)
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
                    flag_xyz(bevy_to_wow(hit.point))
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
        Seen::Terrain { column, row } => {
            let mcnk = row * 16 + column;
            format!("terrain, chunk {column},{row} (MCNK {mcnk}) of {adt}")
        }
        Seen::Doodad { file, unique_id } => format!("doodad, unique id {unique_id}, {file}"),
        Seen::Building {
            file,
            unique_id,
            group,
        } => format!("building, unique id {unique_id}, {file}, group {group}"),
        Seen::Prop {
            file,
            building_file,
            building_unique_id,
            doodad,
        } => format!(
            "{file}, doodad {doodad} of building unique id {building_unique_id}, {building_file}"
        ),
    }
}

fn flag_xyz([x, y, z]: [f32; 3]) -> String {
    format!("{x},{y},{z}")
}

struct Note {
    root: PathBuf,
    name: String,
    pending: PendingNote,
}

impl Note {
    fn write(self, mut frame: Image) -> Result<PathBuf, String> {
        let dir = make_unique_dir(&self.root, &self.name)?;
        ring(&mut frame, self.pending.facts.spot);
        write_png(&frame, &dir.join(FRAME))?;
        let text = dir.join(TEXT);
        std::fs::write(&text, self.pending.text())
            .map_err(|e| format!("writing {}: {e}", text.display()))?;
        Ok(dir)
    }
}

fn make_unique_dir(root: &Path, name: &str) -> Result<PathBuf, String> {
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

#[derive(Debug, PartialEq, Eq)]
struct UtcTime {
    year: u64,
    month: u64,
    day: u64,
    hour: u64,
    minute: u64,
    second: u64,
}

impl UtcTime {
    fn at(since_epoch: Duration) -> Self {
        let secs = since_epoch.as_secs();
        let (days, of_day) = (secs / 86_400, secs % 86_400);
        let z = days + 719_468;
        let era = z / 146_097;
        let of_era = z % 146_097;
        let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
        let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * of_year + 2) / 153;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        Self {
            year: year_of_era + era * 400 + u64::from(month <= 2),
            month,
            day: of_year - (153 * shifted_month + 2) / 5 + 1,
            hour: of_day / 3600,
            minute: of_day % 3600 / 60,
            second: of_day % 60,
        }
    }

    fn dir_name(&self) -> String {
        let Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        } = self;
        format!("{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}Z")
    }

    fn stamp(&self) -> String {
        let Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
        } = self;
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }
}

#[cfg(test)]
mod tests;
