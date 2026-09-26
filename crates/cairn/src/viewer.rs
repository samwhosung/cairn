//! A viewer that stays open: it loads a place once, then answers commands from standard input,
//! one a line, each on a line of standard output, and shoots as `cairn shot` does.

mod command;
mod coverage;
mod cut;
#[cfg(test)]
mod pictures;

use std::fmt::Write as _;
use std::io::BufRead;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use world::collision::CollisionResidency;
use world::sight::Sight;
use world::sight::frame::{SightFramePlugin, SightIndex, SightWanted, sight_camera};
use world::{LeftOut, Residency, WorldCamera};

use crate::fixture::FRAME_STEP;
use crate::shot::{IDENTICAL_CAPTURES, Pipelines, watch_pipelines, write_png};
use crate::view::{Aim, camera};
use command::{Ask, Command};
use coverage::Coverage;
use cut::Cut;

const GIVE_UP_AFTER: Duration = Duration::from_secs(120);
/// Frames a moved camera waits before it trusts that nothing more is on its way: the world's
/// systems see a camera moved at the start of a frame only from that frame on.
const ARRIVAL_FRAMES: u32 = 2;

pub struct ViewerPlugin {
    pub aim: Aim,
    pub size: UVec2,
    pub age: Duration,
}

/// The commands, a line each.
#[derive(Resource)]
pub struct Lines(pub Mutex<Receiver<String>>);

/// Where each answer goes, a line each.
#[derive(Resource)]
pub struct Answers(pub Box<dyn Fn(&str) + Send + Sync>);

/// Standard input's lines, read on a thread of their own, and standard output.
pub fn standard_io() -> (Lines, Answers) {
    let (send, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let answers = Answers(Box::new(|line| println!("{line}")));
    (Lines(Mutex::new(lines)), answers)
}

#[derive(Resource)]
struct Viewer {
    aim: Aim,
    size: UVec2,
    shot: Target,
    sight: Target,
    age_steps: u32,
    aged_at: Option<Aim>,
    opened: Instant,
    step: Step,
}

/// A camera's target image and its size.
struct Target {
    image: Handle<Image>,
    size: UVec2,
}

/// The camera drawing which placement each pixel shows.
#[derive(Component)]
struct SightEye;

enum Step {
    Idle,
    /// The clock held until everything the camera sees has arrived.
    Arriving {
        shot: Option<Shooting>,
        frames: u32,
    },
    /// The world's clock runs `left` frames more.
    Aging {
        shot: Option<Shooting>,
        left: u32,
    },
    /// Frames drawn until [`IDENTICAL_CAPTURES`] in a row come back the same: the shot's, then its
    /// sight frame's when it asks for the list.
    Capturing(Box<Capture>),
}

struct Capture {
    shot: Shooting,
    frame: Frame,
    last: Option<Image>,
    unchanged: u32,
    waiting: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    Shot,
    Sight,
}

struct Shooting {
    ask: Ask,
    size: UVec2,
    asked: Instant,
    aged: bool,
    cut: Option<Cut>,
}

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        let shot = Target::new(app.world_mut().resource_mut::<Assets<Image>>(), self.size);
        let sight = Target::new(app.world_mut().resource_mut::<Assets<Image>>(), self.size);
        let pipelines = watch_pipelines(app);
        let aim = self.aim;
        let (shot_image, sight_image) = (shot.image.clone(), sight.image.clone());
        app.add_plugins(SightFramePlugin)
            .insert_resource(pipelines)
            .insert_resource(Viewer {
                aim,
                size: self.size,
                shot,
                sight,
                age_steps: self.age.div_duration_f32(FRAME_STEP).round() as u32,
                aged_at: None,
                opened: Instant::now(),
                step: Step::Arriving {
                    shot: None,
                    frames: 0,
                },
            })
            .insert_resource(world::CloudClock::Held)
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME_STEP))
            .add_systems(
                Startup,
                move |mut commands: Commands<'_, '_>, mut clock: ResMut<'_, Time<Virtual>>| {
                    clock.pause();
                    commands.spawn((
                        camera(aim.pose()),
                        RenderTarget::Image(shot_image.clone().into()),
                    ));
                    let transform = aim.pose().transform();
                    commands.spawn((sight_camera(transform, sight_image.clone()), SightEye));
                },
            )
            .add_systems(First, take_commands)
            .add_systems(Update, (finish, capture).chain())
            .add_systems(Last, arrive_and_age);
    }
}

impl Target {
    fn new(mut images: Mut<'_, Assets<Image>>, size: UVec2) -> Self {
        let image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
        Self {
            image: images.add(image),
            size,
        }
    }

    /// Made again at `size` when it is another: the camera's new target then.
    fn sized(&mut self, images: &mut Assets<Image>, size: UVec2) -> Option<RenderTarget> {
        if size == self.size {
            return None;
        }
        let image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
        self.image = images.add(image);
        self.size = size;
        Some(RenderTarget::Image(self.image.clone().into()))
    }
}

impl Answers {
    fn say(&self, line: &str) {
        (self.0)(line);
    }
}

#[allow(clippy::too_many_arguments)]
fn take_commands(
    mut commands: Commands<'_, '_>,
    mut viewer: ResMut<'_, Viewer>,
    lines: Option<Res<'_, Lines>>,
    answers: Option<Res<'_, Answers>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut left_out: ResMut<'_, LeftOut>,
    mut camera: Query<'_, '_, (Entity, &mut Transform), With<WorldCamera>>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    let (Some(lines), Some(answers)) = (lines, answers) else {
        return;
    };
    if !matches!(viewer.step, Step::Idle) {
        return;
    }
    let Ok((entity, mut transform)) = camera.single_mut() else {
        return;
    };
    let Ok(lines) = lines.0.lock() else {
        exit.write(AppExit::error());
        return;
    };
    let mut waited = false;
    loop {
        let line = if waited {
            match lines.try_recv() {
                Ok(line) => line,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    exit.write(AppExit::Success);
                    return;
                }
            }
        } else {
            waited = true;
            let Ok(line) = lines.recv() else {
                exit.write(AppExit::Success);
                return;
            };
            line
        };
        let aim = match command::parse(&line) {
            Err(e) => {
                answers.say(&format!("error: {e}"));
                continue;
            }
            Ok(Command::Nothing) => continue,
            Ok(Command::Quit) => {
                exit.write(AppExit::Success);
                return;
            }
            Ok(Command::Where) => {
                answers.say(&format!("ok {}", viewer.aim.flags()));
                continue;
            }
            Ok(Command::Look(aim)) => aim,
            Ok(Command::Move { forward, left, up }) => viewer.aim.moved(forward, left, up),
            Ok(Command::Turn { left, up }) => viewer.aim.turned(left, up),
            Ok(Command::Orbit { left, up, closer }) => match viewer.aim.orbited(left, up, closer) {
                Ok(aim) => aim,
                Err(e) => {
                    answers.say(&format!("error: {e}"));
                    continue;
                }
            },
            Ok(Command::Shot(ask)) => {
                let size = ask.size.unwrap_or(viewer.size);
                if let Some(target) = viewer.shot.sized(&mut images, size) {
                    commands.entity(entity).insert(target);
                }
                if !left_out.0.is_empty() {
                    left_out.0.clear();
                }
                let shot = Shooting {
                    ask,
                    size,
                    asked: Instant::now(),
                    aged: false,
                    cut: None,
                };
                viewer.step = Step::Arriving {
                    shot: Some(shot),
                    frames: 0,
                };
                return;
            }
        };
        viewer.aim = aim;
        *transform = aim.pose().transform();
        answers.say(&format!("ok {}", aim.flags()));
    }
}

#[allow(clippy::too_many_arguments)]
fn arrive_and_age(
    mut viewer: ResMut<'_, Viewer>,
    residency: Res<'_, Residency>,
    collision: Res<'_, CollisionResidency>,
    pipelines: Res<'_, Pipelines>,
    answers: Option<Res<'_, Answers>>,
    mut clock: ResMut<'_, Time<Virtual>>,
    sight: Sight<'_, '_>,
    mut left_out: ResMut<'_, LeftOut>,
) {
    let settled =
        residency.settled() && collision.settled() && pipelines.built.load(Ordering::Relaxed);
    let say = |line: &str| {
        if let Some(answers) = &answers {
            answers.say(line);
        }
    };
    let viewer = &mut *viewer;
    let unaged = viewer.aged_at != Some(viewer.aim) && viewer.age_steps > 0;
    viewer.step = match std::mem::replace(&mut viewer.step, Step::Idle) {
        Step::Arriving { shot, .. }
            if shot
                .as_ref()
                .is_some_and(|s| s.asked.elapsed() > GIVE_UP_AFTER) =>
        {
            say(&format!(
                "error: the world did not arrive in {} s",
                GIVE_UP_AFTER.as_secs()
            ));
            Step::Idle
        }
        Step::Arriving { shot, frames } if !settled || frames + 1 < ARRIVAL_FRAMES => {
            Step::Arriving {
                shot,
                frames: frames + 1,
            }
        }
        Step::Arriving {
            shot: Some(mut shot),
            frames,
        } if shot.cut.is_none() && shot.ask.cuts() => {
            let eye = viewer.aim.pose().eye;
            let cut = cut::cut(&sight, eye, &shot.ask);
            left_out.0.clone_from(&cut.left_out);
            shot.cut = Some(cut);
            Step::Arriving {
                shot: Some(shot),
                frames,
            }
        }
        Step::Arriving { shot, .. } if unaged => {
            clock.unpause();
            Step::Aging {
                shot,
                left: viewer.age_steps,
            }
        }
        Step::Arriving { shot, .. } => {
            viewer.aged_at = Some(viewer.aim);
            aged(viewer, shot, &say)
        }
        Step::Aging { shot, left } if left > 1 => Step::Aging {
            shot,
            left: left - 1,
        },
        Step::Aging { mut shot, .. } => {
            clock.pause();
            viewer.aged_at = Some(viewer.aim);
            if let Some(shot) = &mut shot {
                shot.aged = true;
            }
            aged(viewer, shot, &say)
        }
        step => step,
    };
}

fn aged(viewer: &Viewer, shot: Option<Shooting>, say: &dyn Fn(&str)) -> Step {
    if let Some(shot) = shot {
        return Step::Capturing(Box::new(Capture {
            shot,
            frame: Frame::Shot,
            last: None,
            unchanged: 0,
            waiting: false,
        }));
    }
    let secs = viewer.opened.elapsed().as_secs_f32();
    say(&format!("ready {} in {secs:.3} s", viewer.aim.flags()));
    Step::Idle
}

fn capture(
    mut commands: Commands<'_, '_>,
    mut viewer: ResMut<'_, Viewer>,
    residency: Res<'_, Residency>,
    pipelines: Res<'_, Pipelines>,
    answers: Option<Res<'_, Answers>>,
) {
    let viewer = &mut *viewer;
    let Step::Capturing(capture) = &mut viewer.step else {
        return;
    };
    let failed = if capture.shot.asked.elapsed() > GIVE_UP_AFTER {
        Some(format!("no settled frame in {} s", GIVE_UP_AFTER.as_secs()))
    } else if pipelines.failed.load(Ordering::Relaxed) {
        Some("a render pipeline failed to build; the log says why".to_owned())
    } else {
        None
    };
    if let Some(failed) = failed {
        if let Some(answers) = &answers {
            answers.say(&format!("error: {failed}"));
        }
        viewer.step = Step::Idle;
        return;
    }
    if capture.waiting || !residency.settled() || !pipelines.built.load(Ordering::Relaxed) {
        return;
    }
    capture.waiting = true;
    let target = match capture.frame {
        Frame::Shot => &viewer.shot.image,
        Frame::Sight => &viewer.sight.image,
    };
    commands
        .spawn(Screenshot::image(target.clone()))
        .observe(compare);
}

fn compare(captured: On<'_, '_, ScreenshotCaptured>, mut viewer: ResMut<'_, Viewer>) {
    let Step::Capturing(capture) = &mut viewer.step else {
        return;
    };
    capture.waiting = false;
    let same = capture
        .last
        .as_ref()
        .is_some_and(|last| last.data == captured.image.data);
    if same {
        capture.unchanged += 1;
    } else {
        capture.last = Some(captured.image.clone());
        capture.unchanged = 1;
    }
}

type SightEyes<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Camera,
        &'static mut Transform,
        &'static mut RenderTarget,
    ),
    With<SightEye>,
>;

/// A settled shot written, and its sight frame asked for or counted.
fn finish(
    mut viewer: ResMut<'_, Viewer>,
    answers: Option<Res<'_, Answers>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut eyes: SightEyes<'_, '_>,
    mut wanted: ResMut<'_, SightWanted>,
    index: Res<'_, SightIndex>,
) {
    let viewer = &mut *viewer;
    let Step::Capturing(capture) = &mut viewer.step else {
        return;
    };
    if capture.unchanged < IDENTICAL_CAPTURES {
        return;
    }
    let Some(image) = capture.last.take() else {
        return;
    };
    let answer = match capture.frame {
        Frame::Shot => match write_png(&image, &capture.shot.ask.out) {
            Err(e) => format!("error: {e}"),
            Ok(()) if capture.shot.ask.seen => {
                let Ok((mut camera, mut transform, mut target)) = eyes.single_mut() else {
                    if let Some(answers) = &answers {
                        answers.say("error: the viewer has no camera for the sight frame");
                    }
                    viewer.step = Step::Idle;
                    return;
                };
                if let Some(sized) = viewer.sight.sized(&mut images, capture.shot.size) {
                    *target = sized;
                }
                *transform = viewer.aim.pose().transform();
                camera.is_active = true;
                wanted.0 = true;
                capture.frame = Frame::Sight;
                capture.unchanged = 0;
                return;
            }
            Ok(()) => shot_answer(&capture.shot, None),
        },
        Frame::Sight => {
            if let Ok((mut camera, ..)) = eyes.single_mut() {
                camera.is_active = false;
            }
            wanted.0 = false;
            let rgba = image.data.as_deref().unwrap_or_default();
            let coverage = Coverage::count(rgba, image.width(), &index);
            let list = capture.shot.ask.out.with_extension("txt");
            if let Err(e) = write_png(&image, &capture.shot.ask.out.with_extension("ids.png")) {
                if let Some(answers) = &answers {
                    answers.say(&format!("error: {e}"));
                }
                viewer.step = Step::Idle;
                return;
            }
            let left_out: Vec<u32> = capture
                .shot
                .cut
                .iter()
                .flat_map(|c| c.left_out.iter().copied())
                .collect();
            let text = coverage.text(&viewer.aim.flags(), capture.shot.size, &left_out);
            match std::fs::write(&list, text) {
                Ok(()) => shot_answer(&capture.shot, Some((&list, &coverage))),
                Err(e) => format!("error: writing {}: {e}", list.display()),
            }
        }
    };
    if let Some(answers) = &answers {
        answers.say(&answer);
    }
    viewer.step = Step::Idle;
}

fn shot_answer(shot: &Shooting, seen: Option<(&std::path::Path, &Coverage)>) -> String {
    let secs = shot.asked.elapsed().as_secs_f32();
    let mut answer = format!(
        "ok {} {}x{} in {secs:.3} s",
        shell_path(&shot.ask.out),
        shot.size.x,
        shot.size.y
    );
    if shot.aged {
        answer += ", the world aged first";
    }
    if let Some(cut) = &shot.cut {
        if cut.left_out.is_empty() {
            answer += ", nothing left out";
        } else {
            answer += ", left out";
            for id in &cut.left_out {
                let _ = write!(answer, " {id}");
            }
        }
        if cut.ground_hides {
            answer += ", the ground hides the point cut to";
        }
    }
    if let Some((list, seen)) = seen {
        let _ = write!(answer, "; {}: {}", shell_path(list), seen.summary());
    }
    answer
}

fn shell_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if text.contains(char::is_whitespace) {
        format!("'{text}'")
    } else {
        text
    }
}
