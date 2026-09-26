//! Models drawn one at a time, apart from any place: each from the same angle through an
//! orthographic camera, beside a figure a player's height, on a flat backdrop. The world's clock
//! never runs, so each is drawn at rest, whatever was drawn before it.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::asset::UntypedAssetId;
use bevy::camera::{OrthographicProjection, Projection, RenderTarget, ScalingMode};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use world::coords::wow_to_bevy;
use world::{
    CurrentMap, FullScreenGlow, Install, M2Model, PlacedModel, Placements, Residency, SceneLight,
    TimeOfDay, WmoModel, WorldCamera, WorldSystems,
};

use crate::fixture::FRAME_STEP;
use crate::player::state::CAPSULE_HEIGHT;
use crate::shot::{Pipelines, headless_plugins, watch_pipelines};
use crate::view::Pose;

/// A corner of Azeroth that no tile, horizon or light sphere reaches: the models stand here, lit
/// by the map's own noon.
const STAGE: Vec3 = Vec3::new(16_000.0, 16_000.0, 0.0);
const STAGE_ID: u32 = u32::MAX - 1;
/// The camera looks south-east, down onto the model's front (+x) and its left side (+y), with
/// the sun behind it.
const AZIMUTH: f32 = 225.0;
const ELEVATION: f32 = 25.0;
/// A model larger than this, in yards from its middle to a corner, is drawn scaled down to it,
/// and its figure with it, so all of it lies within the world's far clip. Seen orthographically,
/// the picture is the same.
const STAGE_RADIUS: f32 = 120.0;
const MARGIN: f32 = 0.06;
const BACKDROP: [f32; 3] = [0.64, 0.68, 0.72];
const FIGURE: [f32; 3] = [0.22, 0.26, 0.33];
const FIGURE_BODY_RADIUS: f32 = 0.26;
const FIGURE_BODY_HEIGHT: f32 = 1.64;
const FIGURE_HEAD_RADIUS: f32 = 0.19;
const FIGURE_GAP: f32 = 0.5;
/// Drawn at this many times the picture's size and averaged down, which smooths the edges the
/// client's own frame leaves sharp.
const SUPERSAMPLE: u32 = 2;
const SAME_CAPTURES: u32 = 2;
/// A model not settled by then is drawn as it stands, and said so.
const MODEL_TIMEOUT: Duration = Duration::from_secs(60);
const NOON: u32 = 12 * 60;
const NO_FOG: f32 = 1.0e9;

/// A model to draw, and where its picture goes.
#[derive(Clone, Debug)]
pub struct Sitter {
    /// The install path, as the maps name it.
    pub path: String,
    pub building: bool,
    /// Its box at scale 1 in its own axes: x forward, y left, z up.
    pub bounds: [[f32; 3]; 2],
    pub out: PathBuf,
}

/// How a model's picture came out.
#[derive(Clone, Debug)]
pub struct Drawn {
    pub path: String,
    /// Why the picture may not show the model as it should.
    pub failed: Option<String>,
}

/// Where the camera stands for a model, in the model's own yards with its origin on the stage's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Framing {
    /// The point at the middle of the picture.
    pub target: Vec3,
    pub dist: f32,
    /// Yards across the square picture.
    pub side: f32,
    /// The feet of the figure.
    pub figure: Vec3,
    pub far: f32,
    /// The scale the model and the figure stand at, 1 but for a model too large for the far clip.
    pub stage_scale: f32,
}

/// Right and up across the picture, and forward into it, in WoW's axes.
fn view_axes() -> (Vec3, Vec3, Vec3) {
    let pose = Pose::orbit(Vec3::ZERO, AZIMUTH, ELEVATION, 1.0);
    let forward = (pose.target - pose.eye).normalize();
    let right = forward.cross(Vec3::Z).normalize();
    let up = right.cross(forward);
    (right, up, forward)
}

/// The picture of a model filling `bounds`: the model and, beside it on the ground to its left,
/// the figure, both whole.
pub(crate) fn frame(bounds: [[f32; 3]; 2]) -> Framing {
    let (right, up, forward) = view_axes();
    let [lo, hi] = bounds.map(Vec3::from_array);
    let corners = (0..8).map(|i| {
        Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        )
    });
    let mut points: Vec<Vec3> = corners.collect();
    let left_edge = points
        .iter()
        .map(|c| c.dot(right))
        .fold(f32::INFINITY, f32::min);
    let middle = (lo + hi) * 0.5;
    let ground = Vec3::new(middle.x, middle.y, 0.0);
    let reach = FIGURE_BODY_RADIUS.max(FIGURE_HEAD_RADIUS);
    let figure = ground + right * (left_edge - FIGURE_GAP - reach - ground.dot(right));
    for z in [0.0, CAPSULE_HEIGHT] {
        for side in [-reach, reach] {
            points.push(figure + Vec3::Z * z + right * side);
        }
    }
    let span = |axis: Vec3| {
        points
            .iter()
            .map(|p| p.dot(axis))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), d| {
                (a.min(d), b.max(d))
            })
    };
    let ((u0, u1), (v0, v1)) = (span(right), span(up));
    let side = (u1 - u0).max(v1 - v0).max(0.1) * (1.0 + 2.0 * MARGIN);
    let all_middle = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let radius = points
        .iter()
        .map(|p| p.distance(all_middle))
        .fold(0.0, f32::max)
        .max(0.1);
    let depth = all_middle.dot(forward);
    Framing {
        target: right * f32::midpoint(u0, u1) + up * f32::midpoint(v0, v1) + forward * depth,
        dist: radius + 1.0,
        side,
        figure,
        far: 2.0 * radius + 2.0,
        stage_scale: (STAGE_RADIUS / radius).min(1.0),
    }
}

/// Draws each model into its picture, `side` pixels square, in the order given.
pub fn draw(install: &Install, sitters: Vec<Sitter>, side: u32) -> Result<Vec<Drawn>, String> {
    let map = CurrentMap::find(&install.0, "Azeroth")?;
    let mut app = App::new();
    world::register_source(&mut app, install);
    app.add_plugins(headless_plugins())
        .insert_resource(map)
        .insert_resource(TimeOfDay { minute: NOON })
        .insert_resource(FullScreenGlow(false))
        .insert_resource(world::CloudClock::Held)
        .insert_resource(world::WholeBuildings(true))
        .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME_STEP))
        .add_plugins((
            world::collision::CollisionPlugin,
            world::LoadersPlugin,
            world::WorldPlugin,
        ));
    let pipelines = watch_pipelines(&mut app);
    let size = side * SUPERSAMPLE;
    let image = Image::new_target_texture(size, size, TextureFormat::Rgba8UnormSrgb, None);
    let target = app.world_mut().resource_mut::<Assets<Image>>().add(image);
    let drawn = Arc::new(Mutex::new(Vec::new()));
    app.insert_resource(pipelines)
        .insert_resource(Studio {
            queue: sitters.into(),
            sitting: None,
            target: target.clone(),
            side,
            drawn: drawn.clone(),
            captured: None,
        })
        .add_systems(Startup, move |mut commands: Commands<'_, '_>| {
            commands.spawn((
                world::world_camera(Transform::default()),
                RenderTarget::Image(target.clone().into()),
            ));
        })
        .add_systems(Startup, (hold_clock, spawn_figure))
        .add_systems(
            Update,
            (sit.before(WorldSystems), backdrop.after(WorldSystems)),
        );
    if let AppExit::Error(code) = app.run() {
        return Err(format!("the drawing stopped with code {code}"));
    }
    let drawn = drawn.lock().map_err(|e| e.to_string())?;
    Ok(drawn.clone())
}

#[derive(Resource)]
struct Studio {
    queue: VecDeque<Sitter>,
    sitting: Option<Sitting>,
    target: Handle<Image>,
    side: u32,
    drawn: Arc<Mutex<Vec<Drawn>>>,
    captured: Option<Vec<u8>>,
}

struct Sitting {
    sitter: Sitter,
    handle: UntypedAssetId,
    since: Instant,
    frames: u32,
    capturing: bool,
    last: Option<Vec<u8>>,
    same: u32,
}

#[derive(Component)]
struct Figure;

type FigureQuery<'w, 's> = Query<
    'w,
    's,
    (&'static mut Transform, &'static mut Visibility),
    (With<Figure>, Without<WorldCamera>),
>;

fn hold_clock(mut clock: ResMut<'_, Time<Virtual>>) {
    clock.pause();
}

fn spawn_figure(
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<StandardMaterial>>,
) {
    let [r, g, b] = FIGURE;
    let material = materials.add(StandardMaterial {
        base_color: Color::linear_rgb(r, g, b),
        unlit: true,
        ..StandardMaterial::default()
    });
    let body = meshes.add(Capsule3d::new(
        FIGURE_BODY_RADIUS,
        FIGURE_BODY_HEIGHT - 2.0 * FIGURE_BODY_RADIUS,
    ));
    let head = meshes.add(Sphere::new(FIGURE_HEAD_RADIUS));
    commands
        .spawn((Figure, Transform::default(), Visibility::Hidden))
        .with_children(|figure| {
            figure.spawn((
                Mesh3d(body),
                MeshMaterial3d(material.clone()),
                Transform::from_xyz(0.0, FIGURE_BODY_HEIGHT * 0.5, 0.0),
            ));
            figure.spawn((
                Mesh3d(head),
                MeshMaterial3d(material),
                Transform::from_xyz(0.0, CAPSULE_HEIGHT - FIGURE_HEAD_RADIUS, 0.0),
            ));
        });
}

fn stage_point(framing: &Framing, local: Vec3) -> Vec3 {
    wow_to_bevy((STAGE + local * framing.stage_scale).to_array())
}

/// Stands the next model on the stage, or waits for the one there to settle and draws it.
#[allow(clippy::too_many_arguments)]
fn sit(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    residency: Res<'_, Residency>,
    pipelines: Res<'_, Pipelines>,
    mut studio: ResMut<'_, Studio>,
    mut placements: ResMut<'_, Placements>,
    mut camera: Query<'_, '_, (&mut Transform, &mut Projection), With<WorldCamera>>,
    mut figure: FigureQuery<'_, '_>,
    mut exit: MessageWriter<'_, AppExit>,
) {
    let studio = &mut *studio;
    let Some(sitting) = &mut studio.sitting else {
        match studio.queue.pop_front() {
            Some(sitter) => {
                let sitting = seat(sitter, &server, &mut placements, &mut camera, &mut figure);
                commands.insert_resource(world::rig::AnimRng::default());
                studio.sitting = Some(sitting);
            }
            None => {
                exit.write(AppExit::Success);
            }
        }
        return;
    };
    sitting.frames += 1;
    if let Some(bytes) = studio.captured.take() {
        sitting.capturing = false;
        if sitting.last.as_ref() == Some(&bytes) {
            sitting.same += 1;
        } else {
            sitting.last = Some(bytes);
            sitting.same = 1;
        }
    }
    let timed_out = sitting.since.elapsed() > MODEL_TIMEOUT;
    if sitting.same >= SAME_CAPTURES || (timed_out && sitting.last.is_some()) {
        let failed = server
            .load_state(sitting.handle)
            .is_failed()
            .then(|| "the file did not load".to_owned())
            .or_else(|| timed_out.then(|| "it did not settle in time".to_owned()));
        let bytes = sitting.last.take().unwrap_or_default();
        let big = studio.side * SUPERSAMPLE;
        let written = survey::save_averaged(&bytes, big, SUPERSAMPLE, &sitting.sitter.out);
        if let Ok(mut drawn) = studio.drawn.lock() {
            drawn.push(Drawn {
                path: sitting.sitter.path.clone(),
                failed: written.err().or(failed),
            });
        }
        placements.lift(STAGE_ID);
        studio.sitting = None;
        return;
    }
    let settled = residency.settled() && pipelines.built.load(Ordering::Relaxed);
    if sitting.frames > 2 && (settled || timed_out) && !sitting.capturing {
        sitting.capturing = true;
        commands
            .spawn(Screenshot::image(studio.target.clone()))
            .observe(keep_capture);
    }
}

fn seat(
    sitter: Sitter,
    server: &AssetServer,
    placements: &mut Placements,
    camera: &mut Query<'_, '_, (&mut Transform, &mut Projection), With<WorldCamera>>,
    figure: &mut FigureQuery<'_, '_>,
) -> Sitting {
    let framing = frame(sitter.bounds);
    let (model, handle) = if sitter.building {
        let url = world::wmo_url(&sitter.path);
        let handle: Handle<WmoModel> = server.load(&url);
        let model = PlacedModel::Building {
            url,
            doodad_set: 0,
            name_set: 0,
        };
        (model, handle.id().untyped())
    } else {
        let url = world::m2_url(&sitter.path);
        let handle: Handle<M2Model> = server.load(&url);
        (PlacedModel::Doodad { url }, handle.id().untyped())
    };
    let s = framing.stage_scale;
    let at =
        Transform::from_translation(stage_point(&framing, Vec3::ZERO)).with_scale(Vec3::splat(s));
    placements.place(STAGE_ID, model, at);
    if let Ok((mut t, mut projection)) = camera.single_mut() {
        *t = Pose::orbit(
            STAGE + framing.target * s,
            AZIMUTH,
            ELEVATION,
            framing.dist * s,
        )
        .transform();
        *projection = Projection::Orthographic(OrthographicProjection {
            near: 0.0,
            far: framing.far * s,
            scaling_mode: ScalingMode::Fixed {
                width: framing.side * s,
                height: framing.side * s,
            },
            ..OrthographicProjection::default_3d()
        });
    }
    if let Ok((mut t, mut visibility)) = figure.single_mut() {
        *t = Transform::from_translation(stage_point(&framing, framing.figure))
            .with_scale(Vec3::splat(s));
        *visibility = Visibility::Inherited;
    }
    Sitting {
        sitter,
        handle,
        since: Instant::now(),
        frames: 0,
        capturing: false,
        last: None,
        same: 0,
    }
}

fn keep_capture(captured: On<'_, '_, ScreenshotCaptured>, mut studio: ResMut<'_, Studio>) {
    studio.captured = Some(captured.image.data.clone().unwrap_or_default());
}

/// The world's light as it samples it, with no fog, no sky bodies and one colour behind.
fn backdrop(mut light: ResMut<'_, SceneLight>, mut clear: ResMut<'_, ClearColor>) {
    let light = &mut *light;
    light.fog_color = BACKDROP;
    light.fog_start = NO_FOG;
    light.fog_end = NO_FOG;
    light.room_fog = world::Fog {
        color: BACKDROP,
        start: NO_FOG,
        end: NO_FOG,
    };
    light.sky = [BACKDROP; 5];
    light.sky_warp = 0.0;
    light.star_alpha = 0.0;
    light.cloud_density = 0.0;
    light.sun_disc_scale = 0.0;
    light.sun_flare = 0.0;
    light.moon_disc_scale = 0.0;
    light.moon_flare = 0.0;
    light.moon02_disc_scale = 0.0;
    let [r, g, b] = BACKDROP;
    clear.0 = Color::linear_rgb(r, g, b);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_figure_stands_to_the_left_on_the_ground_and_both_fit() {
        let lamp = frame([[-0.11, -0.57, -0.01], [0.29, 1.71, 4.09]]);
        let (right, up, _) = view_axes();
        assert!(lamp.figure.z.abs() < 1e-6, "on the ground");
        let figure_u = lamp.figure.dot(right);
        let lamp_left = [-0.11f32, 0.29]
            .iter()
            .flat_map(|&x| [-0.57f32, 1.71].map(|y| Vec3::new(x, y, 0.0).dot(right)))
            .fold(f32::INFINITY, f32::min);
        assert!(figure_u < lamp_left - FIGURE_GAP, "left of the lamp");
        let top = Vec3::new(0.29, 1.71, 4.09).dot(up);
        let half = lamp.side / 2.0;
        assert!(
            top < lamp.target.dot(up) + half,
            "the lamp's top is in the picture"
        );
        assert!((lamp.stage_scale - 1.0).abs() < f32::EPSILON);
        let city = frame([[-600.0, -600.0, 0.0], [600.0, 600.0, 200.0]]);
        assert!(city.stage_scale < 0.2 && city.far * city.stage_scale < 350.0);
    }
}
