//! A headless client driven by scripted keys at a fixed step.

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

use crate::net::{Net, NetPlugin};
use crate::player::state::Player;
use crate::player::{Mode, PlayerPlugin};
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
    last_frame_began: Option<Instant>,
}

pub fn time_update(joined: bool, step: Duration) -> TimeUpdateStrategy {
    if joined {
        TimeUpdateStrategy::Automatic
    } else {
        TimeUpdateStrategy::ManualDuration(step)
    }
}

fn settle_all(walkers: &mut [&mut Walker]) {
    let deadline = Instant::now() + Duration::from_secs(300);
    hold_game_clocks(walkers);
    while !walkers.iter().all(|w| w.settled()) {
        assert!(Instant::now() < deadline, "the collision never settled");
        round(walkers);
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
    for w in walkers.iter_mut() {
        w.pace();
    }
}

fn round(walkers: &mut [&mut Walker]) {
    for w in walkers.iter_mut() {
        w.app.update();
    }
    std::thread::sleep(walkers.iter().map(|w| w.step).max().unwrap_or_default());
}

impl Walker {
    /// A client on `map` whose body starts with its feet at `feet` (WoW), facing `heading_deg`
    /// (0 north, 90 west), stepped at `hz`. `None` without `WOW_DATA`.
    pub fn new(map: &str, feet: [f32; 3], heading_deg: f32, hz: f32) -> Option<Self> {
        Self::build(map, feet, heading_deg, hz, None, None)
    }

    pub fn dressed(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        Self::build(map, feet, heading_deg, hz, Some(look), None)
    }

    /// A client on Azeroth that joins the server at `server` as `look` and stands where its
    /// welcome places it, stepped at `hz` and paced to the wall clock.
    pub fn joined(server: SocketAddr, name: &str, look: CharacterLook, hz: f32) -> Option<Self> {
        let mut walker = Self::welcomed(server, name, look, hz)?;
        ready(&mut [&mut walker]);
        Some(walker)
    }

    /// [`Walker::joined`], but not yet [`ready`].
    pub fn welcomed(server: SocketAddr, name: &str, look: CharacterLook, hz: f32) -> Option<Self> {
        let hello = crate::net::hello(name.to_owned(), &look);
        let net = Net::connect(server, hello);
        Self::build("Azeroth", [0.0; 3], 0.0, hz, None, Some(net))
    }

    fn pace(&mut self) {
        self.last_frame_began = Some(Instant::now());
    }

    pub fn settled(&self) -> bool {
        !self.player().settling && self.app.world().resource::<CollisionResidency>().settled()
    }

    fn build(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        dressed: Option<CharacterLook>,
        net: Option<Net>,
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
        let step = Duration::from_secs_f64(1.0 / f64::from(hz));
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, TransformPlugin, InputPlugin));
        world::register_source(&mut app, &install);
        app.add_plugins(AssetPlugin::default())
            .init_asset::<Image>()
            .init_asset::<Mesh>()
            .init_asset::<StandardMaterial>()
            .add_plugins(world::LoadersPlugin)
            .insert_resource(current)
            .insert_resource(time_update(net.is_some(), step))
            .add_plugins((
                CollisionPlugin,
                PlayerPlugin {
                    pose: Pose::orbit(Vec3::from_array(feet), heading_deg, 12.0, 16.0),
                    mode: Mode::Walk,
                    look: dressed.unwrap_or_else(|| CharacterLook::naked(1, 0)),
                },
            ));
        if let Some(tables) = tables {
            app.insert_resource(tables);
        }
        let joining = net.is_some();
        if let Some(net) = net {
            app.insert_resource(net).add_plugins(NetPlugin);
        }
        app.finish();
        app.cleanup();
        let mut walker = Self {
            app,
            step,
            last_frame_began: None,
        };
        if joining {
            walker.await_welcome();
        } else {
            walker.settle();
        }
        Some(walker)
    }

    fn await_welcome(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.net().is_none_or(|n| n.welcome().is_none()) {
            assert!(Instant::now() < deadline, "no welcome from the server");
            self.app.update();
            std::thread::sleep(self.step);
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

    fn grounded(mut self, xy: [f32; 2]) -> Self {
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

    /// Puts the feet at a WoW position, at rest, and settles there.
    pub fn teleport(&mut self, wow: Vec3) {
        let mut player = self.app.world_mut().resource_mut::<Player>();
        player.pos = wow_to_bevy(wow.to_array());
        player.vel_y = 0.0;
        player.horiz_vel = Vec3::ZERO;
        player.airborne_since = None;
        player.settling = true;
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
                if let Some(last) = &mut self.last_frame_began {
                    std::thread::sleep(
                        (*last + self.step).saturating_duration_since(Instant::now()),
                    );
                    *last = Instant::now();
                }
                self.app.update();
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
        self.app.update();
        e
    }

    pub fn remove(&mut self, e: Entity) {
        self.app.world_mut().despawn(e);
        self.app.update();
    }
}
