//! Pictures of the walker from its own follow camera: the window's plugins drawn headless into an
//! image, the body walked by scripted keys at a fixed step, and a PNG taken with the clocks held.
//! They need a GPU as well as the install, so they run only when asked for, writing into the
//! directory `CAIRN_PICTURES` names.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::camera::RenderTarget;
use bevy::ecs::system::RunSystemOnce;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::time::TimeUpdateStrategy;
use world::collision::{CollisionPlugin, WorldCollision};
use world::coords::wow_to_bevy;
use world::rig::{AnimParked, RigPose, RigSkin};
use world::unit::{BodyDressed, CharacterLook, CharacterTables, UnitBody};
use world::{CurrentMap, Install, Residency, TimeOfDay, WorldCamera};

use crate::player::camera::{CameraControl, CameraRig};
use crate::player::state::Player;
use crate::player::{Mode, PlayerBody, PlayerPlugin};
use crate::shot::{Pipelines, headless_plugins, watch_pipelines, write_png};
use crate::view::Pose;

const STEP: Duration = Duration::from_nanos(16_666_667);
const SIZE: UVec2 = UVec2::new(1280, 720);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
const FRAMES_TO_REACH_THE_IMAGE: usize = 3;

const GOLDSHIRE: [f32; 2] = [-9439.1, 51.2];
const EAST: f32 = 270.0;
const HILLTOP_SOUTH_OF_GOLDSHIRE: [f32; 2] = [-9200.0, -420.0];
const SUN_BEARING: f32 = 45.0;
struct Stand {
    xy: [f32; 2],
    heading: f32,
}

const FACING_A_GOLDSHIRE_LAMPPOST: Stand = Stand {
    xy: [-9433.0, 44.0],
    heading: 215.0,
};
const FACING_A_LAMPPOST_BELOW_THE_ABBEY: Stand = Stand {
    xy: [-8952.0, -113.0],
    heading: 320.0,
};

struct Painter {
    app: App,
    target: Handle<Image>,
    out: PathBuf,
}

impl Painter {
    /// Headings in degrees: 0 north, 90 west.
    fn new(xy: [f32; 2], heading_deg: f32, look: CharacterLook) -> Option<Self> {
        let (Some(data), Some(out)) = (
            std::env::var_os("WOW_DATA"),
            std::env::var_os("CAIRN_PICTURES"),
        ) else {
            eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
            return None;
        };
        let install = Install::open(&PathBuf::from(data)).expect("open the install");
        let map = CurrentMap::find(&install.0, "Azeroth").expect("the map");
        let tables = CharacterTables::load(&install).expect("the character tables");
        let mut app = App::new();
        world::register_source(&mut app, &install);
        app.add_plugins(headless_plugins())
            .insert_resource(map)
            .insert_resource(tables)
            .insert_resource(TimeOfDay { minute: 12 * 60 })
            .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
            .add_plugins((
                CollisionPlugin,
                PlayerPlugin {
                    pose: Pose::orbit(Vec3::new(xy[0], xy[1], 500.0), heading_deg, 12.0, 16.0),
                    mode: Mode::Walk,
                    look,
                },
                world::LoadersPlugin,
                world::WorldPlugin,
            ));
        let pipelines = watch_pipelines(&mut app);
        app.insert_resource(pipelines);
        app.finish();
        app.cleanup();
        let target =
            app.world_mut()
                .resource_mut::<Assets<Image>>()
                .add(Image::new_target_texture(
                    SIZE.x,
                    SIZE.y,
                    TextureFormat::Rgba8UnormSrgb,
                    None,
                ));
        let mut painter = Self {
            app,
            target,
            out: PathBuf::from(out),
        };
        painter.app.update();
        let camera = painter
            .app
            .world_mut()
            .query_filtered::<Entity, With<WorldCamera>>()
            .single(painter.app.world())
            .expect("the follow camera");
        let view = RenderTarget::Image(painter.target.clone().into());
        painter.app.world_mut().entity_mut(camera).insert(view);
        painter.put_on_ground(xy);
        painter.settle();
        Some(painter)
    }

    fn put_on_ground(&mut self, xy: [f32; 2]) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while self.app.world().resource::<Player>().settling {
            assert!(Instant::now() < deadline, "the collision never settled");
            self.app.update();
            std::thread::sleep(STEP);
        }
        let from = wow_to_bevy([xy[0], xy[1], 500.0]);
        let ground = self
            .app
            .world_mut()
            .run_system_once(move |c: WorldCollision<'_, '_>| {
                c.ray_body(from, Dir3::NEG_Y, 2000.0)
                    .map(|h| from.y - h.distance)
            })
            .expect("the system runs")
            .expect("ground under the start");
        let mut player = self.app.world_mut().resource_mut::<Player>();
        player.pos = Vec3::new(from.x, ground, from.z);
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
        player.airborne_since = None;
        player.settling = true;
    }

    fn settle(&mut self) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while !self.arrived() {
            assert!(Instant::now() < deadline, "the world never arrived");
            self.app.update();
            std::thread::sleep(STEP);
        }
    }

    fn arrived(&mut self) -> bool {
        let world = self.app.world_mut();
        let pipelines = world.resource::<Pipelines>();
        assert!(
            !pipelines.failed.load(Ordering::Relaxed),
            "a render pipeline failed"
        );
        if world.resource::<Player>().settling
            || !world.resource::<Residency>().settled()
            || !pipelines.built.load(Ordering::Relaxed)
        {
            return false;
        }
        let Ok(body) = world
            .query_filtered::<&UnitBody, (With<PlayerBody>, With<BodyDressed>)>()
            .single(world)
        else {
            return false;
        };
        let images = world.resource::<Assets<Image>>();
        body.character.iter().all(|c| {
            [&c.body, &c.hair, &c.skin_extra]
                .into_iter()
                .flatten()
                .all(|h| images.contains(h))
        })
    }

    fn key(&mut self, key_code: KeyCode, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn orbit(&mut self, yaw_by: f32, distance: f32) {
        let mut control = self.app.world_mut().resource_mut::<CameraControl>();
        control.distance = distance;
        control.target_distance = distance;
        let world = self.app.world_mut();
        let mut rig = world
            .query_filtered::<&mut CameraRig, With<WorldCamera>>()
            .single_mut(world)
            .expect("the follow camera");
        rig.yaw += yaw_by;
    }

    fn set_time(&mut self, hour: u32, minute: u32) {
        self.app.world_mut().resource_mut::<TimeOfDay>().minute = hour * 60 + minute;
    }

    fn tilt_up(&mut self, radians: f32) {
        let world = self.app.world_mut();
        let mut rig = world
            .query_filtered::<&mut CameraRig, With<WorldCamera>>()
            .single_mut(world)
            .expect("the follow camera");
        rig.pitch = radians;
    }

    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.app.update();
        }
    }

    fn wait(&mut self, secs: f32) {
        self.run((secs / STEP.as_secs_f32()).round() as usize);
    }

    fn shoot(&mut self, name: &str) {
        self.app.world_mut().resource_mut::<Time<Virtual>>().pause();
        self.run(FRAMES_TO_REACH_THE_IMAGE);
        let shot: Arc<Mutex<Option<Image>>> = Arc::default();
        let into = shot.clone();
        self.app
            .world_mut()
            .spawn(Screenshot::image(self.target.clone()))
            .observe(move |captured: On<'_, '_, ScreenshotCaptured>| {
                *into.lock().expect("the shot") = Some(captured.image.clone());
            });
        let deadline = Instant::now() + Duration::from_secs(30);
        while shot.lock().expect("the shot").is_none() {
            assert!(Instant::now() < deadline, "no frame came back");
            self.app.update();
        }
        let image = shot.lock().expect("the shot").take().expect("a frame");
        let path = self.out.join(format!("{name}.png"));
        write_png(&image, &path).expect("the picture writes");
        eprintln!("wrote {}", path.display());
        self.app
            .world_mut()
            .resource_mut::<Time<Virtual>>()
            .unpause();
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_stands_runs_and_jumps_in_goldshire() {
    let Some(mut p) = Painter::new(GOLDSHIRE, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.wait(2.0);
    p.shoot("goldshire-1-standing");
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    p.wait(1.5);
    p.shoot("goldshire-2-running");
    p.key(KeyCode::Space, ButtonState::Pressed);
    p.run(1);
    p.key(KeyCode::Space, ButtonState::Released);
    p.wait(0.25);
    p.shoot("goldshire-3-jumping");
    p.wait(1.0);
    p.key(KeyCode::KeyW, ButtonState::Released);
    p.wait(0.5);
    p.shoot("goldshire-4-landed");
    p.orbit(std::f32::consts::PI, 4.0);
    p.wait(1.0);
    p.shoot("goldshire-5-face");
    p.orbit(0.0, 1.2);
    p.wait(1.0);
    p.shoot("goldshire-6-fading");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_walker_under_the_sky_at_dawn_noon_dusk_and_night() {
    let hill = HILLTOP_SOUTH_OF_GOLDSHIRE;
    let Some(mut p) = Painter::new(hill, SUN_BEARING, CharacterLook::naked(1, 0)) else {
        return;
    };
    for (name, hour, minute, tilt) in [
        ("sky-1-dawn", 6, 30, 0.1),
        ("sky-2-noon", 12, 0, 0.35),
        ("sky-3-dusk", 20, 15, 0.1),
        ("sky-4-night", 0, 30, 0.6),
    ] {
        p.set_time(hour, minute);
        p.tilt_up(tilt);
        p.wait(2.0);
        p.shoot(name);
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_doodads_move_in_goldshire_and_before_the_abbey() {
    for (place, stand) in [
        ("goldshire", FACING_A_GOLDSHIRE_LAMPPOST),
        ("abbey", FACING_A_LAMPPOST_BELOW_THE_ABBEY),
    ] {
        let Some(mut p) = Painter::new(stand.xy, stand.heading, CharacterLook::naked(1, 0)) else {
            return;
        };
        p.orbit(0.0, 8.0);
        p.tilt_up(-0.1);
        p.wait(2.0);
        for i in 0..4 {
            p.shoot(&format!("{place}-doodads-{i}"));
            p.wait(0.4);
        }
    }
}

fn frame_costs(p: &mut Painter, frames: usize) -> String {
    let mut costs: Vec<Duration> = (0..frames)
        .map(|_| {
            let t = Instant::now();
            p.app.update();
            t.elapsed()
        })
        .collect();
    costs.sort();
    let at = |q: f32| costs[((costs.len() - 1) as f32 * q) as usize].as_secs_f64() * 1e3;
    let mean = costs.iter().sum::<Duration>().as_secs_f64() * 1e3 / costs.len() as f64;
    format!(
        "mean {mean:.3} ms, p50 {:.3}, p90 {:.3}, p99 {:.3}, max {:.3}",
        at(0.5),
        at(0.9),
        at(0.99),
        at(1.0)
    )
}

#[test]
#[ignore = "a measurement, for a release build on a GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_frame_cost_of_goldshire() {
    let Some(mut p) = Painter::new(GOLDSHIRE, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    p.wait(2.0);
    let standing = frame_costs(&mut p, 600);
    let rigs = rig_census(&mut p);
    p.key(KeyCode::KeyW, ButtonState::Pressed);
    let running = frame_costs(&mut p, 1200);
    eprintln!("goldshire: standing {standing} with {rigs}; running {running}");
}

fn rig_census(p: &mut Painter) -> String {
    let world = p.app.world_mut();
    let mut rigs = world.query::<(&RigPose, Has<AnimParked>, Has<RigSkin>)>();
    let (mut n, mut parked, mut skinned) = (0, 0, 0);
    for (_, is_parked, has_skin) in rigs.iter(world) {
        n += 1;
        parked += usize::from(is_parked);
        skinned += usize::from(has_skin);
    }
    format!("{n} rigs, {parked} parked, {skinned} with a palette slot")
}
