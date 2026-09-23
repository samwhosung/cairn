use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bevy::app::{PluginGroupBuilder, ScheduleRunnerPlugin};
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::render::{Render, RenderApp, RenderPlugin, RenderSystems};
use bevy::shader::PipelineCacheError;
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;

use world::Residency;

use crate::view::{Pose, camera};

const IDENTICAL_CAPTURES: u32 = 3;
const TIMEOUT: Duration = Duration::from_secs(120);

pub fn headless_plugins() -> PluginGroupBuilder {
    DefaultPlugins
        .set(WindowPlugin {
            primary_window: None,
            exit_condition: ExitCondition::DontExit,
            close_when_requested: false,
            ..WindowPlugin::default()
        })
        .set(RenderPlugin {
            synchronous_pipeline_compilation: true,
            ..RenderPlugin::default()
        })
        .disable::<WinitPlugin>()
        .add(ScheduleRunnerPlugin::run_loop(Duration::ZERO))
}

pub struct ShotPlugin {
    pub pose: Pose,
    pub size: UVec2,
    pub out: PathBuf,
}

#[derive(Resource)]
struct Shot {
    out: PathBuf,
    target: Handle<Image>,
    started: Instant,
    capturing: bool,
    last: Option<Vec<u8>>,
    unchanged: u32,
}

/// A frame drawn while a pipeline still waits for its shaders silently lacks that pipeline's
/// draws.
#[derive(Resource, Clone, Default)]
struct Pipelines {
    built: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
}

impl Plugin for ShotPlugin {
    fn build(&self, app: &mut App) {
        let image = Image::new_target_texture(
            self.size.x,
            self.size.y,
            TextureFormat::Rgba8UnormSrgb,
            None,
        );
        let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let (pose, view_target) = (self.pose, target.clone());
        let pipelines = Pipelines::default();
        app.sub_app_mut(RenderApp)
            .insert_resource(pipelines.clone())
            .add_systems(Render, watch_pipelines.in_set(RenderSystems::Cleanup));
        app.insert_resource(pipelines)
            .insert_resource(Shot {
                out: self.out.clone(),
                target,
                started: Instant::now(),
                capturing: false,
                last: None,
                unchanged: 0,
            })
            .add_systems(Startup, move |mut commands: Commands<'_, '_>| {
                commands.spawn((
                    camera(pose),
                    RenderTarget::Image(view_target.clone().into()),
                ));
            })
            .add_systems(Update, capture);
    }
}

fn watch_pipelines(cache: Res<'_, PipelineCache>, pipelines: Res<'_, Pipelines>) {
    let failed = cache.pipelines().any(|pipeline| {
        matches!(
            pipeline.state,
            CachedPipelineState::Err(
                PipelineCacheError::ProcessShaderError(_)
                    | PipelineCacheError::CreateShaderModule(_)
            )
        )
    });
    pipelines.failed.store(failed, Ordering::Relaxed);
    let built = cache.waiting_pipelines().next().is_none();
    pipelines.built.store(built, Ordering::Relaxed);
}

fn capture(
    mut commands: Commands<'_, '_>,
    residency: Res<'_, Residency>,
    pipelines: Res<'_, Pipelines>,
    mut shot: ResMut<'_, Shot>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    if shot.started.elapsed() > TIMEOUT {
        fail(
            &mut exit,
            &format!("no settled frame in {} s", TIMEOUT.as_secs()),
        );
        return;
    }
    if pipelines.failed.load(Ordering::Relaxed) {
        fail(
            &mut exit,
            "a render pipeline failed to build; the log says why",
        );
        return;
    }
    if shot.capturing || !residency.settled() || !pipelines.built.load(Ordering::Relaxed) {
        return;
    }
    shot.capturing = true;
    commands
        .spawn(Screenshot::image(shot.target.clone()))
        .observe(compare);
}

fn compare(
    captured: On<'_, '_, ScreenshotCaptured>,
    mut shot: ResMut<'_, Shot>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    shot.capturing = false;
    if shot.last.is_some() && shot.last == captured.image.data {
        shot.unchanged += 1;
    } else {
        shot.last.clone_from(&captured.image.data);
        shot.unchanged = 1;
    }
    if shot.unchanged < IDENTICAL_CAPTURES {
        return;
    }
    match write_png(&captured.image, &shot.out) {
        Ok(()) => {
            println!("cairn: wrote {}", shot.out.display());
            exit.write(AppExit::Success);
        }
        Err(e) => fail(&mut exit, &e),
    }
}

fn write_png(image: &Image, out: &Path) -> Result<(), String> {
    let pixels = image
        .clone()
        .try_into_dynamic()
        .map_err(|e| e.to_string())?;
    pixels
        .to_rgb8()
        .save(out)
        .map_err(|e| format!("writing {}: {e}", out.display()))
}

fn fail(exit: &mut MessageWriter<'_, AppExit>, message: &str) {
    eprintln!("cairn: {message}");
    exit.write(AppExit::error());
}
