//! A headless client driven by scripted keys at a fixed step.

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
use world::{CurrentMap, Install};

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
}

impl Walker {
    /// A client on `map` whose body starts with its feet at `feet` (WoW), facing `heading_deg`
    /// (0 north, 90 west), stepped at `hz`. `None` without `WOW_DATA`.
    pub fn new(map: &str, feet: [f32; 3], heading_deg: f32, hz: f32) -> Option<Self> {
        Self::build(map, feet, heading_deg, hz, None)
    }

    pub fn dressed(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        look: CharacterLook,
    ) -> Option<Self> {
        Self::build(map, feet, heading_deg, hz, Some(look))
    }

    fn build(
        map: &str,
        feet: [f32; 3],
        heading_deg: f32,
        hz: f32,
        dressed: Option<CharacterLook>,
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
            .insert_resource(TimeUpdateStrategy::ManualDuration(step))
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
        app.finish();
        app.cleanup();
        let mut walker = Self { app, step };
        walker.settle();
        Some(walker)
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

    /// Updates until the body is let go and the collision around it is resident: under load the
    /// body's own settle can give up on its stall clock first. That clock is the game's, so each
    /// update waits out its own step on the wall clock.
    pub fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(300);
        while self.player().settling || !self.app.world().resource::<CollisionResidency>().settled()
        {
            assert!(Instant::now() < deadline, "the collision never settled");
            self.app.update();
            std::thread::sleep(self.step);
        }
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
