//! A viewer that stays open: it loads a place once, then answers commands from standard input,
//! one a line, each on a line of standard output, and shoots as `cairn shot` does.

mod command;
mod coverage;
mod cut;
mod hands;
mod palette;
#[cfg(test)]
mod pictures;

use std::collections::BTreeSet;
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
    step: Step,
}

struct Target {
    image: Handle<Image>,
    size: UVec2,
}

#[derive(Component)]
struct SightFrameCamera;

enum Step {
    Idle,
    Arriving {
        shot: Option<Shooting>,
    },
    Aging {
        shot: Option<Shooting>,
        frames_left: u32,
    },
    Capturing(Box<Capture>),
    Handling(Box<hands::Handling>),
    Palette(Box<palette::Asking>),
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
        app.add_plugins((world::hands::HandsPlugin, world::hands::SightPickingPlugin))
            .add_systems(Startup, |mut commands: Commands<'_, '_>| {
                commands.spawn(hands::VIEWER_POINTER);
                commands.spawn(hands::PICK_POINTER);
            })
            .add_systems(Last, hands::handle);
        app.add_plugins(SightFramePlugin)
            .insert_resource(pipelines)
            .insert_resource(Viewer {
                aim,
                size: self.size,
                shot,
                sight,
                age_steps: self.age.div_duration_f32(FRAME_STEP).round() as u32,
                aged_at: None,
                step: Step::Arriving { shot: None },
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
                    commands.spawn((
                        sight_camera(transform, sight_image.clone()),
                        SightFrameCamera,
                    ));
                },
            )
            .add_systems(First, wait_for_commands)
            .add_systems(Update, (finish, capture).chain())
            .add_systems(
                Last,
                (
                    (arrive_and_age, leave_out_the_running_cut).chain(),
                    palette::answer,
                ),
            );
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

    fn resize(&mut self, images: &mut Assets<Image>, size: UVec2) -> Option<RenderTarget> {
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

type WorldCameras<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Transform, &'static mut GlobalTransform),
    With<WorldCamera>,
>;

#[allow(clippy::too_many_arguments)]
fn wait_for_commands(
    mut commands: Commands<'_, '_>,
    mut viewer: ResMut<'_, Viewer>,
    lines: Option<Res<'_, Lines>>,
    answers: Option<Res<'_, Answers>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut camera: WorldCameras<'_, '_>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    let (Some(lines), Some(answers)) = (lines, answers) else {
        return;
    };
    if !matches!(viewer.step, Step::Idle) {
        return;
    }
    let Ok((entity, mut transform, mut global)) = camera.single_mut() else {
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
            Ok(Command::Move {
                forward_yd,
                left_yd,
                up_yd,
            }) => viewer.aim.moved(forward_yd, left_yd, up_yd),
            Ok(Command::Turn { left_deg, up_deg }) => viewer.aim.turned(left_deg, up_deg),
            Ok(Command::Orbit {
                left_deg,
                up_deg,
                closer_yd,
            }) => match viewer.aim.orbited(left_deg, up_deg, closer_yd) {
                Ok(aim) => aim,
                Err(e) => {
                    answers.say(&format!("error: {e}"));
                    continue;
                }
            },
            Ok(Command::Hands(ask)) => {
                viewer.step = Step::Handling(Box::new(hands::Handling::new(ask)));
                return;
            }
            Ok(Command::Palette(ask)) => {
                viewer.step = Step::Palette(Box::new(palette::Asking::new(ask)));
                return;
            }
            Ok(Command::Shot(ask)) => {
                let size = ask.size.unwrap_or(viewer.size);
                if let Some(target) = viewer.shot.resize(&mut images, size) {
                    commands.entity(entity).insert(target);
                }
                let shot = Shooting {
                    ask,
                    size,
                    asked: Instant::now(),
                    aged: false,
                    cut: None,
                };
                viewer.step = Step::Arriving { shot: Some(shot) };
                return;
            }
        };
        viewer.aim = aim;
        *transform = aim.pose().transform();
        *global = GlobalTransform::from(*transform);
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
        Step::Arriving { shot } if !settled => Step::Arriving { shot },
        Step::Arriving {
            shot: Some(mut shot),
        } if shot.cut.is_none() && shot.ask.leaves_any_out() => {
            let eye = viewer.aim.pose().eye;
            shot.cut = Some(cut::cut(&sight, eye, &shot.ask));
            Step::Arriving { shot: Some(shot) }
        }
        Step::Arriving { shot } if unaged => {
            clock.unpause();
            Step::Aging {
                shot,
                frames_left: viewer.age_steps,
            }
        }
        Step::Arriving { shot, .. } => {
            viewer.aged_at = Some(viewer.aim);
            aged(viewer, shot, &say)
        }
        Step::Aging { shot, frames_left } if frames_left > 1 => Step::Aging {
            shot,
            frames_left: frames_left - 1,
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
    say(&format!("ready {}", viewer.aim.flags()));
    Step::Idle
}

fn capture(
    mut commands: Commands<'_, '_>,
    mut viewer: ResMut<'_, Viewer>,
    residency: Res<'_, Residency>,
    pipelines: Res<'_, Pipelines>,
    answers: Option<Res<'_, Answers>>,
    palette: Option<Res<'_, crate::palette::Settled>>,
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
    let palette_settled = palette.is_none_or(|p| p.0);
    if capture.waiting
        || !residency.settled()
        || !pipelines.built.load(Ordering::Relaxed)
        || !palette_settled
    {
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

type SightFrameCameras<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Camera,
        &'static mut Transform,
        &'static mut RenderTarget,
    ),
    With<SightFrameCamera>,
>;

fn finish(
    mut viewer: ResMut<'_, Viewer>,
    answers: Option<Res<'_, Answers>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut cameras: SightFrameCameras<'_, '_>,
    mut wanted: ResMut<'_, SightWanted>,
    index: Res<'_, SightIndex>,
) {
    let Viewer {
        aim, sight, step, ..
    } = &mut *viewer;
    let Step::Capturing(capture) = step else {
        return;
    };
    if capture.unchanged < IDENTICAL_CAPTURES {
        return;
    }
    let Some(image) = capture.last.take() else {
        return;
    };
    let answer = match capture.frame {
        Frame::Shot => {
            let sight_frame = (sight, &mut *images, &mut cameras, &mut *wanted);
            match finish_shot(capture, &image, *aim, sight_frame) {
                Finished::Answer(answer) => answer,
                Finished::SightFrameNext => return,
            }
        }
        Frame::Sight => finish_sight(capture, &image, *aim, &index, &mut cameras, &mut wanted),
    };
    if let Some(answers) = &answers {
        answers.say(&answer);
    }
    *step = Step::Idle;
}

fn leave_out_the_running_cut(viewer: Res<'_, Viewer>, mut left_out: ResMut<'_, LeftOut>) {
    let cut = match &viewer.step {
        Step::Arriving { shot: Some(shot) }
        | Step::Aging {
            shot: Some(shot), ..
        } => shot.cut.as_ref(),
        Step::Capturing(capture) => capture.shot.cut.as_ref(),
        _ => None,
    };
    let nothing = BTreeSet::new();
    let wanted = cut.map_or(&nothing, |c| &c.left_out);
    if left_out.0 != *wanted {
        left_out.0.clone_from(wanted);
    }
}

type SightFrame<'a, 'w, 's> = (
    &'a mut Target,
    &'a mut Assets<Image>,
    &'a mut SightFrameCameras<'w, 's>,
    &'a mut SightWanted,
);

enum Finished {
    Answer(String),
    SightFrameNext,
}

fn finish_shot(
    capture: &mut Capture,
    image: &Image,
    aim: Aim,
    (sight, images, cameras, wanted): SightFrame<'_, '_, '_>,
) -> Finished {
    if let Err(e) = write_png(image, &capture.shot.ask.out) {
        return Finished::Answer(format!("error: {e}"));
    }
    if !capture.shot.ask.seen {
        return Finished::Answer(shot_answer(&capture.shot, None));
    }
    let Ok((mut camera, mut transform, mut target)) = cameras.single_mut() else {
        return Finished::Answer("error: the viewer has no camera for the sight frame".into());
    };
    if let Some(resized) = sight.resize(images, capture.shot.size) {
        *target = resized;
    }
    *transform = aim.pose().transform();
    camera.is_active = true;
    wanted.0 = true;
    capture.frame = Frame::Sight;
    capture.unchanged = 0;
    Finished::SightFrameNext
}

fn finish_sight(
    capture: &Capture,
    image: &Image,
    aim: Aim,
    index: &SightIndex,
    cameras: &mut SightFrameCameras<'_, '_>,
    wanted: &mut SightWanted,
) -> String {
    if let Ok((mut camera, ..)) = cameras.single_mut() {
        camera.is_active = false;
    }
    wanted.0 = false;
    let rgba = image.data.as_deref().unwrap_or_default();
    let coverage = Coverage::count(rgba, image.width(), index);
    let out = &capture.shot.ask.out;
    if let Err(e) = write_png(image, &out.with_extension("ids.png")) {
        return format!("error: {e}");
    }
    let left_out: Vec<u32> = capture
        .shot
        .cut
        .iter()
        .flat_map(|c| c.left_out.iter().copied())
        .collect();
    let list = out.with_extension("txt");
    let text = coverage.listing(&aim.flags(), capture.shot.size, &left_out);
    match std::fs::write(&list, text) {
        Ok(()) => shot_answer(&capture.shot, Some((&list, &coverage))),
        Err(e) => format!("error: writing {}: {e}", list.display()),
    }
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
        if cut.ground_hides_cut_to {
            answer += ", the ground hides the point cut to";
        }
    }
    if let Some((list, seen)) = seen {
        let _ = write!(answer, "; {}: {}", shell_path(list), seen.summary_line());
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
