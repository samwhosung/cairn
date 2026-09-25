//! The window the pictures are drawn in: the window's plugins, headless on the GPU, drawing into an
//! image from its own follow camera.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::camera::{ImageRenderTarget, RenderTarget};
use bevy::ecs::system::RunSystemOnce;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::window::{PrimaryWindow, WindowResolution};
use server::Standing;
use world::collision::{CollisionPlugin, CollisionResidency, WorldCollision};
use world::coords::wow_to_bevy;
use world::rig::{AnimParked, RigPose, RigSkin};
use world::unit::{BodyDressed, CharacterLook, CharacterTables, UnitBody};
use world::{CurrentMap, Install, Residency, TimeOfDay, WorldCamera};

use super::alone::{self, Pace};
use super::clock::{Frames, SharedClock};
use super::walker::{Through, time_update};
use crate::net::{Net, NetPlugin};
use crate::note::NotePlugin;
use crate::player::camera::{CameraControl, CameraRig};
use crate::player::flags::FALLING;
use crate::player::state::Player;
use crate::player::{Mode, PlayerBody, PlayerPlugin, Teleported};
use crate::shot::{Pipelines, headless_plugins, watch_pipelines, write_png};
use crate::view::Pose;

pub(super) const STEP: Duration = Duration::from_nanos(16_666_667);
pub(super) const SIZE: UVec2 = UVec2::new(1280, 720);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
const FRAMES_TO_REACH_THE_IMAGE: usize = 3;

pub(super) struct Painter {
    pub(super) app: App,
    target: Handle<Image>,
    out: PathBuf,
    pace: Pace,
    judge_on_drop: bool,
    frames: Frames,
    beside: Vec<Box<dyn FnMut()>>,
}

impl Painter {
    /// Headings in degrees: 0 north, 90 west.
    pub(super) fn new(xy: [f32; 2], heading_deg: f32, look: CharacterLook) -> Option<Self> {
        Self::standing(
            xy,
            heading_deg,
            look,
            Some(Through::ItsOwn { record: None }),
        )
    }

    pub(super) fn standing(
        xy: [f32; 2],
        heading_deg: f32,
        look: CharacterLook,
        through: Option<Through>,
    ) -> Option<Self> {
        let mut painter = Self::build([xy[0], xy[1], 500.0], heading_deg, look, through)?;
        painter.stand_on(xy);
        Some(painter)
    }

    /// A painter joining `server`, standing on the ground where its welcome places it once its
    /// world has arrived there; `feet` is where it starts until then.
    pub(super) fn joined(
        server: SocketAddr,
        feet: [f32; 3],
        heading_deg: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        let through = Through::Loopback {
            addr: server,
            name: "Painter".into(),
            look: look.clone(),
        };
        Self::welcomed(through, feet, heading_deg, look)
    }

    /// [`Painter::joined`], but as a guest of the server on `clock`.
    pub(super) fn on_clock(
        clock: &SharedClock,
        feet: [f32; 3],
        heading_deg: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        let through = Through::Clock {
            clock: clock.clone(),
            name: "Painter".into(),
            look: look.clone(),
            standing: Standing::Guest,
        };
        Self::welcomed(through, feet, heading_deg, look)
    }

    /// A painter hosting the server on `clock`, which judges it once it is dropped.
    pub(super) fn hosting(
        clock: &SharedClock,
        feet: [f32; 3],
        heading_deg: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        let through = Through::Clock {
            clock: clock.clone(),
            name: "Host".into(),
            look: look.clone(),
            standing: Standing::Host,
        };
        Self::welcomed(through, feet, heading_deg, look)
    }

    fn welcomed(
        through: Through,
        feet: [f32; 3],
        heading_deg: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        let mut painter = Self::build(feet, heading_deg, look, Some(through))?;
        let spawn = painter
            .app
            .world()
            .get_resource::<Net>()
            .and_then(Net::welcome)
            .expect("a welcome")
            .spawn
            .pos;
        painter.stand_on([spawn[0], spawn[1]]);
        Some(painter)
    }

    pub(super) fn build(
        feet: [f32; 3],
        heading_deg: f32,
        look: CharacterLook,
        through: Option<Through>,
    ) -> Option<Self> {
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
        let pose = Pose::orbit(Vec3::from_array(feet), heading_deg, 12.0, 16.0);
        let over_loopback = matches!(through, Some(Through::Loopback { .. }));
        let judge_on_drop = through.as_ref().is_some_and(Through::hosted);
        let (net, frames) = match through {
            Some(through) => {
                let (net, frames) = through.join(map.id, pose, &look, STEP);
                (Some(net), frames)
            }
            None => (None, Frames::OnTheWall),
        };
        let mut app = App::new();
        world::register_source(&mut app, &install);
        app.add_plugins(headless_plugins())
            .insert_resource(map)
            .insert_resource(tables)
            .insert_resource(TimeOfDay { minute: 12 * 60 })
            .insert_resource(time_update(over_loopback, STEP))
            .add_plugins((
                CollisionPlugin,
                PlayerPlugin {
                    pose,
                    mode: Mode::Walk,
                    look,
                },
                world::LoadersPlugin,
                world::WorldPlugin,
                NotePlugin {
                    dir: Some(PathBuf::from(&out).join("notes")),
                },
            ));
        let joins = net.is_some();
        if let Some(net) = net {
            app.insert_resource(net).add_plugins(NetPlugin);
        }
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
            pace: Pace::default(),
            judge_on_drop,
            frames,
            beside: Vec::new(),
        };
        painter.hold();
        let camera = painter
            .app
            .world_mut()
            .query_filtered::<Entity, With<WorldCamera>>()
            .single(painter.app.world())
            .expect("the follow camera");
        let view = RenderTarget::Image(painter.target.clone().into());
        painter.app.world_mut().entity_mut(camera).insert(view);
        if joins {
            painter.await_welcome();
        }
        if !over_loopback && painter.frames.clock().is_none() {
            painter.pace.start();
        }
        Some(painter)
    }

    pub(super) fn draw_for_a_window_at(&mut self, pixels_to_the_point: u32) -> UVec2 {
        let px = SIZE * pixels_to_the_point;
        let frame = Image::new_target_texture(px.x, px.y, TextureFormat::Bgra8UnormSrgb, None);
        self.target = self
            .app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(frame);
        let scale = pixels_to_the_point as f32;
        let view = RenderTarget::Image(ImageRenderTarget {
            handle: self.target.clone(),
            scale_factor: scale,
        });
        let world = self.app.world_mut();
        let camera = world
            .query_filtered::<Entity, With<WorldCamera>>()
            .single(world)
            .expect("the follow camera");
        world.entity_mut(camera).insert(view);
        let resolution = WindowResolution::new(px.x, px.y).with_scale_factor_override(scale);
        let window = Window {
            resolution,
            ..Window::default()
        };
        world.spawn((window, PrimaryWindow));
        px
    }

    fn await_welcome(&mut self) {
        self.clock().pause();
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while self
            .app
            .world()
            .get_resource::<Net>()
            .and_then(Net::welcome)
            .is_none()
        {
            assert!(Instant::now() < deadline, "no welcome from the server");
            self.frame();
            self.frames.wait(STEP);
        }
    }

    /// Runs `window`, another client on this painter's clock, a frame after each of the painter's
    /// own that moves the clock on.
    pub(super) fn beside(&mut self, window: impl FnMut() + 'static) {
        self.beside.push(Box::new(window));
    }

    fn frame(&mut self) {
        self.frames.frame(&mut self.app);
        for window in &mut self.beside {
            window();
        }
    }

    pub(super) fn hold(&mut self) {
        self.frames.hold(&mut self.app);
    }

    pub(super) fn clock(&mut self) -> Mut<'_, Time<Virtual>> {
        self.app.world_mut().resource_mut::<Time<Virtual>>()
    }

    pub(super) fn stand_on(&mut self, xy: [f32; 2]) {
        self.clock().pause();
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while self.app.world().resource::<Player>().settling {
            assert!(Instant::now() < deadline, "the collision never settled");
            self.hold();
            self.frames.wait(STEP);
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
        self.app.world_mut().write_message(Teleported);
        self.settle();
        self.clock().unpause();
    }

    pub(super) fn settle(&mut self) {
        let deadline = Instant::now() + LOAD_TIMEOUT;
        while !self.arrived() {
            assert!(Instant::now() < deadline, "the world never arrived");
            self.hold();
            self.frames.wait(STEP);
        }
    }

    pub(super) fn arrived(&mut self) -> bool {
        let world = self.app.world_mut();
        let pipelines = world.resource::<Pipelines>();
        assert!(
            !pipelines.failed.load(Ordering::Relaxed),
            "a render pipeline failed"
        );
        let player = world.resource::<Player>();
        if player.settling
            || player.move_flags & FALLING != 0
            || !world.resource::<CollisionResidency>().settled()
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

    pub(super) fn key(&mut self, key_code: KeyCode, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    pub(super) fn orbit(&mut self, yaw_by: f32, distance: f32) {
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

    pub(super) fn set_time(&mut self, hour: u32, minute: u32) {
        self.app.world_mut().resource_mut::<TimeOfDay>().minute = hour * 60 + minute;
    }

    pub(super) fn tilt_up(&mut self, radians: f32) {
        let world = self.app.world_mut();
        let mut rig = world
            .query_filtered::<&mut CameraRig, With<WorldCamera>>()
            .single_mut(world)
            .expect("the follow camera");
        rig.pitch = radians;
    }

    pub(super) fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            self.pace.wait(STEP);
            self.frame();
        }
    }

    pub(super) fn timed_frame(&mut self) -> Duration {
        self.pace.wait(STEP);
        let t = Instant::now();
        self.frame();
        t.elapsed()
    }

    pub(super) fn wait(&mut self, secs: f32) {
        self.run((secs / STEP.as_secs_f32()).round() as usize);
    }

    pub(super) fn shoot(&mut self, name: &str) {
        self.clock().pause();
        for _ in 0..FRAMES_TO_REACH_THE_IMAGE {
            if self.frames.clock().is_some() {
                self.hold();
            } else {
                self.run(1);
            }
        }
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
            self.hold();
        }
        let image = shot.lock().expect("the shot").take().expect("a frame");
        let path = self.out.join(format!("{name}.png"));
        write_png(&image, &path).expect("the picture writes");
        eprintln!("wrote {}", path.display());
        self.clock().unpause();
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        if self.judge_on_drop {
            alone::assert_honest(&mut self.app, self.frames.clock(), "painter");
        }
    }
}

pub(super) fn frame_costs(p: &mut Painter, frames: usize) -> String {
    let mut costs: Vec<Duration> = (0..frames).map(|_| p.timed_frame()).collect();
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

pub(super) fn rig_census(p: &mut Painter) -> String {
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
