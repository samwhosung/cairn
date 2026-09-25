//! A headless client driven by scripted keys at a fixed step, through its own server as a bare
//! window is, or through another's.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, CollisionLayers, RigidBody};
use bevy::asset::AssetPlugin;
use bevy::ecs::system::RunSystemOnce;
use bevy::input::ButtonState;
use bevy::input::InputPlugin;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::transform::TransformPlugin;
use world::collision::{CollisionPlugin, CollisionResidency, Liquids, WorldCollision};
use world::coords::{bevy_to_wow, wow_to_bevy};
use world::unit::{CharacterLook, CharacterTables};
use world::{CurrentMap, Install, WorldCamera};

use super::alone::{self, Judged, Pace};
use super::clock::{self, Served, Stepping, step_at};
use crate::net::{Net, NetPlugin, hello};
use crate::player::state::Player;
use crate::player::{Mode, PlayerPlugin, Teleported};
use crate::view::Pose;

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// Feet, WoW coordinates.
    pub wow: [f32; 3],
    pub vel_y: f32,
    pub flags: u32,
}

pub struct Walker {
    pub app: App,
    step: Duration,
    pace: Pace,
    judge_on_drop: bool,
    stepping: Option<Stepping>,
}

pub enum Through {
    /// Its own server, as its host, on a clock of its own that its frames step.
    ItsOwn { record: Option<PathBuf> },
    /// Its own server on the wall clock, as a bare window runs one.
    ItsOwnInRealTime,
    Loopback {
        addr: SocketAddr,
        name: String,
        look: CharacterLook,
    },
    /// The server on a test's clock, as its host or a guest.
    Clock {
        clock: Served,
        name: String,
        look: CharacterLook,
        host: bool,
    },
}

impl Through {
    /// Whether the window hosts the server, which judges it once it is dropped.
    pub fn hosted(&self) -> bool {
        matches!(
            self,
            Self::ItsOwn { .. } | Self::ItsOwnInRealTime | Self::Clock { host: true, .. }
        )
    }

    /// The window's connection on `map`, standing at `pose` as `look`, and the clock its frames
    /// step by `step` when it has one.
    pub fn join(
        self,
        map: u32,
        pose: Pose,
        look: &CharacterLook,
        step: Duration,
    ) -> (Net, Option<Stepping>) {
        let on = |clock: &Served, host: bool, hello| {
            let server = clock.borrow_mut().connect(host);
            (Net::in_process(server, hello), Some(Stepping::on(clock)))
        };
        match self {
            Self::ItsOwn { record } => {
                let clock = clock::serve(&alone::own_config(map, pose, record), step);
                on(&clock, true, hello("Walker".into(), look))
            }
            Self::ItsOwnInRealTime => (alone::own_server(map, pose, look), None),
            Self::Loopback { addr, name, look } => (Net::connect(addr, hello(name, &look)), None),
            Self::Clock {
                clock,
                name,
                look,
                host,
            } => on(&clock, host, hello(name, &look)),
        }
    }
}

pub fn time_update(over_loopback: bool, step: Duration) -> TimeUpdateStrategy {
    if over_loopback {
        TimeUpdateStrategy::Automatic
    } else {
        TimeUpdateStrategy::ManualDuration(step)
    }
}

/// On a test's clock, a settle holds the clock too, so that a world's streaming takes no test time.
fn settle_all(walkers: &mut [&mut Walker]) {
    let deadline = Instant::now() + Duration::from_secs(300);
    hold_game_clocks(walkers);
    while !walkers.iter().all(|w| w.settled()) {
        assert!(Instant::now() < deadline, "the collision never settled");
        for w in walkers.iter_mut() {
            w.hold();
        }
        wait_a_step(walkers);
    }
    release_game_clocks(walkers);
}

/// Unheld, a body's own settle can give up and let the body go before a slow world has streamed
/// in.
fn hold_game_clocks(walkers: &mut [&mut Walker]) {
    for w in walkers.iter_mut() {
        w.app.world_mut().resource_mut::<Time<Virtual>>().pause();
    }
}

fn release_game_clocks(walkers: &mut [&mut Walker]) {
    for w in walkers.iter_mut() {
        w.app.world_mut().resource_mut::<Time<Virtual>>().unpause();
    }
}

/// The physics steps by the game clock the settle held, so the first round after it takes in
/// every collider that streamed in meanwhile; a second keeps that round's length out of the first
/// paced frame.
pub fn ready(walkers: &mut [&mut Walker]) {
    settle_all(walkers);
    round(walkers);
    round(walkers);
    for w in walkers.iter_mut().filter(|w| w.stepping.is_none()) {
        w.pace.start();
    }
}

fn round(walkers: &mut [&mut Walker]) {
    for w in walkers.iter_mut() {
        w.frame();
    }
    wait_a_step(walkers);
}

fn wait_a_step(walkers: &[&mut Walker]) {
    let on_the_wall = walkers.iter().filter(|w| w.stepping.is_none());
    std::thread::sleep(on_the_wall.map(|w| w.step).max().unwrap_or_default());
}

impl Walker {
    /// A client on `map` whose body starts with its feet at `feet` (WoW), facing `heading_deg`
    /// (0 north, 90 west), stepped at `hz`. `None` without `WOW_DATA`.
    pub fn new(map: &str, feet: [f32; 3], heading_deg: f32, hz: f32) -> Option<Self> {
        Self::build(
            map,
            feet,
            heading_deg,
            hz,
            None,
            Through::ItsOwn { record: None },
        )
    }

    pub fn dressed(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        Self::build(
            map,
            feet,
            heading_deg,
            hz,
            Some(look),
            Through::ItsOwn { record: None },
        )
    }

    /// [`Walker::new`] on Azeroth, its server writing every input to `log`.
    pub fn recorded(feet: [f32; 3], heading_deg: f32, hz: f32, log: PathBuf) -> Option<Self> {
        Self::build(
            "Azeroth",
            feet,
            heading_deg,
            hz,
            None,
            Through::ItsOwn { record: Some(log) },
        )
    }

    /// A client on Azeroth that joins the server on `clock` as a guest of `look` and stands where
    /// its welcome places it, stepped with the clock.
    pub fn joined(clock: &Served, name: &str, look: CharacterLook) -> Option<Self> {
        let mut walker = Self::welcomed(clock, name, look)?;
        ready(&mut [&mut walker]);
        Some(walker)
    }

    /// [`Walker::joined`], but not yet [`ready`].
    pub fn welcomed(clock: &Served, name: &str, look: CharacterLook) -> Option<Self> {
        let hz = 1.0 / clock.borrow().step().as_secs_f32();
        let through = Through::Clock {
            clock: clock.clone(),
            name: name.to_owned(),
            look,
            host: false,
        };
        let mut walker = Self::build("Azeroth", [0.0; 3], 0.0, hz, None, through)?;
        walker.await_welcome();
        Some(walker)
    }

    pub fn settled(&self) -> bool {
        self.net().is_none_or(|n| n.welcome().is_some())
            && !self.player().settling
            && self.app.world().resource::<CollisionResidency>().settled()
    }

    pub fn stop_and_judge(&mut self) -> Option<Judged> {
        self.judge_on_drop = false;
        alone::judge(&mut self.app, self.stepping.as_ref().map(Stepping::clock))
    }

    fn build(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        dressed: Option<CharacterLook>,
        through: Through,
    ) -> Option<Self> {
        let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
            eprintln!("skipped: WOW_DATA is not set");
            return None;
        };
        let install = Install::open(&data).expect("open the install");
        let tables = dressed
            .is_some()
            .then(|| CharacterTables::load(&install).expect("the character tables"));
        let current = CurrentMap::find(&install.0, map).expect("the map");
        let step = step_at(hz);
        let pose = Pose::orbit(Vec3::from_array(feet), heading_deg, 12.0, 16.0);
        let look = dressed.unwrap_or_else(|| CharacterLook::naked(1, 0));
        let over_loopback = matches!(through, Through::Loopback { .. });
        let judge_on_drop = through.hosted();
        let (net, stepping) = through.join(current.id, pose, &look, step);
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, InputPlugin));
        world::register_source(&mut app, &install);
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins(world::LoadersPlugin)
            .insert_resource(current)
            .insert_resource(time_update(over_loopback, step))
            .add_plugins((
                CollisionPlugin,
                PlayerPlugin {
                    pose,
                    mode: Mode::Walk,
                    look,
                },
            ))
            .insert_resource(net)
            .add_plugins(NetPlugin);
        if let Some(tables) = tables {
            app.insert_resource(tables);
        }
        app.finish();
        app.cleanup();
        let mut walker = Self {
            app,
            step,
            pace: Pace::default(),
            judge_on_drop,
            stepping,
        };
        if judge_on_drop {
            walker.await_welcome();
            walker.settle();
            if walker.stepping.is_none() {
                walker.pace.start();
            }
        }
        Some(walker)
    }

    /// Frames with the game clock held until the server's welcome arrives.
    fn await_welcome(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        hold_game_clocks(&mut [&mut *self]);
        while self.net().is_none_or(|n| n.welcome().is_none()) {
            assert!(Instant::now() < deadline, "no welcome from the server");
            self.frame();
            wait_a_step(&[&mut *self]);
        }
        release_game_clocks(&mut [&mut *self]);
    }

    /// A frame a step on from the last, on the test's clock or the wall's.
    fn frame(&mut self) {
        match &mut self.stepping {
            Some(stepping) => stepping.frame(&mut self.app),
            None => self.app.update(),
        }
    }

    /// A frame that moves a test's clock nothing on.
    fn hold(&mut self) {
        match &mut self.stepping {
            Some(stepping) => stepping.hold(&mut self.app),
            None => self.app.update(),
        }
    }

    pub fn net(&self) -> Option<&Net> {
        self.app.world().get_resource::<Net>()
    }

    /// On `Azeroth`, with the feet placed on the ground under `xy`.
    pub fn on_ground(xy: [f32; 2], heading_deg: f32, hz: f32) -> Option<Self> {
        Some(Self::new("Azeroth", [xy[0], xy[1], 500.0], heading_deg, hz)?.grounded(xy))
    }

    pub fn dressed_on_ground(
        xy: [f32; 2],
        heading_deg: f32,
        hz: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        Some(Self::dressed("Azeroth", [xy[0], xy[1], 500.0], heading_deg, hz, look)?.grounded(xy))
    }

    pub fn grounded(mut self, xy: [f32; 2]) -> Self {
        let ground = self
            .ground_under(xy[0], xy[1], 500.0)
            .expect("ground under the point");
        self.teleport(Vec3::new(xy[0], xy[1], ground));
        self
    }

    pub fn settle(&mut self) {
        settle_all(&mut [self]);
    }

    pub fn player(&self) -> &Player {
        self.app.world().resource::<Player>()
    }

    pub fn wow(&self) -> [f32; 3] {
        bevy_to_wow(self.player().pos)
    }

    /// Puts the feet at a WoW position, at rest, asks the server to put them there too, and settles
    /// there.
    pub fn teleport(&mut self, wow: Vec3) {
        let mut player = self.app.world_mut().resource_mut::<Player>();
        player.pos = wow_to_bevy(wow.to_array());
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
        player.airborne_since = None;
        player.settling = true;
        self.app.world_mut().write_message(Teleported);
        self.settle();
    }

    /// Flies, as Ctrl+Shift+F does, with the camera over `xy` until the collision there has come,
    /// and returns the ground under it.
    pub fn fly_over(&mut self, xy: [f32; 2]) -> f32 {
        self.chord(KeyCode::KeyF);
        assert_eq!(*self.app.world().resource::<Mode>(), Mode::Fly);
        self.put_camera([xy[0], xy[1], 500.0]);
        self.settle();
        self.ground_under(xy[0], xy[1], 500.0)
            .expect("ground under the point")
    }

    /// Lands where `wow` is, as Ctrl+Shift+G does, flying, with the camera there.
    pub fn land(&mut self, wow: [f32; 3]) {
        self.put_camera(wow);
        self.chord(KeyCode::KeyG);
        assert_eq!(*self.app.world().resource::<Mode>(), Mode::Walk);
    }

    fn chord(&mut self, key: KeyCode) {
        self.press(KeyCode::ControlLeft);
        self.press(KeyCode::ShiftLeft);
        self.tap(key);
        self.release(KeyCode::ShiftLeft);
        self.release(KeyCode::ControlLeft);
    }

    fn put_camera(&mut self, wow: [f32; 3]) {
        let world = self.app.world_mut();
        world
            .query_filtered::<&mut Transform, With<WorldCamera>>()
            .single_mut(world)
            .expect("the camera")
            .translation = wow_to_bevy(wow);
    }

    /// The highest front-facing ground under a WoW column, from `from_z` down.
    pub fn ground_under(&mut self, x: f32, y: f32, from_z: f32) -> Option<f32> {
        let from = wow_to_bevy([x, y, from_z]);
        self.app
            .world_mut()
            .run_system_once(move |c: WorldCollision<'_, '_>| {
                c.ray_body(from, Dir3::NEG_Y, 2000.0)
                    .map(|h| from.y - h.distance)
            })
            .expect("the system runs")
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

    pub fn press(&mut self, key: KeyCode) {
        self.key(key, ButtonState::Pressed);
    }

    pub fn release(&mut self, key: KeyCode) {
        self.key(key, ButtonState::Released);
    }

    pub fn tap(&mut self, key: KeyCode) -> Frame {
        self.press(key);
        let frame = self.run(1)[0];
        self.release(key);
        frame
    }

    pub fn run(&mut self, n: usize) -> Vec<Frame> {
        (0..n)
            .map(|_| {
                self.pace.wait(self.step);
                self.frame();
                let p = self.player();
                Frame {
                    wow: bevy_to_wow(p.pos),
                    vel_y: p.vel_y,
                    flags: p.move_flags,
                }
            })
            .collect()
    }

    /// Turns the aim to a heading, as the mouse would.
    pub fn aim(&mut self, heading_deg: f32) {
        self.app.world_mut().resource_mut::<Player>().face_yaw = heading_deg.to_radians();
    }

    pub fn pitch(&mut self, deg: f32) {
        self.app.world_mut().resource_mut::<Player>().mover_pitch = deg.to_radians();
    }

    pub fn net_mut(&mut self) -> Option<Mut<'_, Net>> {
        self.app.world_mut().get_resource_mut::<Net>()
    }

    /// The first front face along a WoW-space ray, as `(distance, normal in WoW axes)`.
    pub fn ray(&mut self, from: [f32; 3], dir: [f32; 3], max: f32) -> Option<(f32, [f32; 3])> {
        self.ray_hit(from, dir, max).map(|(d, n, _)| (d, n))
    }

    /// [`Self::ray`], with the collision layers the face belongs to.
    pub fn ray_hit(
        &mut self,
        from: [f32; 3],
        dir: [f32; 3],
        max: f32,
    ) -> Option<(f32, [f32; 3], u32)> {
        let origin = wow_to_bevy(from);
        let dir = Dir3::new(wow_to_bevy(dir)).expect("a direction");
        self.app
            .world_mut()
            .run_system_once(
                move |c: WorldCollision<'_, '_>, l: Query<'_, '_, &CollisionLayers>| {
                    c.ray_body(origin, dir, max).map(|h| {
                        let layers = l.get(h.entity).map_or(u32::MAX, |l| l.memberships.0);
                        (h.distance, bevy_to_wow(h.normal), layers)
                    })
                },
            )
            .expect("the system runs")
    }

    /// The liquid surface over a WoW column, if any.
    pub fn water(&mut self, wow: [f32; 3]) -> Option<f32> {
        self.app
            .world_mut()
            .run_system_once(move |l: Liquids<'_, '_>| l.liquid_at(wow).map(|h| h.surface_z))
            .expect("the system runs")
    }

    /// A solid block between two WoW corners, baked in world space and wound outward as the
    /// streamed geometry is.
    pub fn block(&mut self, lo: [f32; 3], hi: [f32; 3]) -> Entity {
        let (a, b) = (wow_to_bevy(lo), wow_to_bevy(hi));
        let (min, max) = (a.min(b), a.max(b));
        let corners = (0..8)
            .map(|i| {
                Vec3::new(
                    if i & 1 == 0 { min.x } else { max.x },
                    if i & 2 == 0 { min.y } else { max.y },
                    if i & 4 == 0 { min.z } else { max.z },
                )
            })
            .collect();
        let faces = vec![
            [0, 2, 1],
            [1, 2, 3],
            [4, 5, 6],
            [5, 7, 6],
            [0, 1, 5],
            [0, 5, 4],
            [2, 6, 7],
            [2, 7, 3],
            [0, 4, 6],
            [0, 6, 2],
            [1, 3, 7],
            [1, 7, 5],
        ];
        let e = self
            .app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                RigidBody::Static,
                Collider::trimesh(corners, faces),
            ))
            .id();
        self.frame();
        e
    }

    pub fn remove(&mut self, e: Entity) {
        self.app.world_mut().despawn(e);
        self.frame();
    }
}

impl Drop for Walker {
    fn drop(&mut self) {
        if self.judge_on_drop {
            let clock = self.stepping.as_ref().map(Stepping::clock);
            alone::assert_honest(&mut self.app, clock, "walker");
        }
    }
}
