mod colliders;

use std::time::Duration;

use bevy::asset::AssetPlugin;
use bevy::ecs::system::RunSystemOnce;
use bevy::input::keyboard::{Key, KeyboardInput, NativeKey};
use bevy::input::mouse::{MouseButtonInput, MouseMotion};
use bevy::input::{ButtonState, InputPlugin};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bevy::transform::TransformPlugin;
use world::collision::{CollisionPlugin, WorldCollision};

use super::camera::{CameraControl, CameraRig, LOGIN_PITCH};
use super::camera_dynamics::CameraOptions;
use super::fixture::player_capsule;
use super::state::Player;
use super::{PlayerCapsule, body, controller};

const HZ: f32 = 60.0;
const START: Vec3 = Vec3::new(0.0, 0.0, 16.0);
const RECORDED_PATH: u64 = 0xfe3d_1d94_20d7_76e6;

#[derive(Clone, Copy)]
enum Act {
    Press(KeyCode),
    Release(KeyCode),
    Button(MouseButton, bool),
    Tap(KeyCode),
    Wait { secs: f32 },
    Drag { rate: f32, secs: f32 },
    Steer { to: [f32; 2], reach: f32, secs: f32 },
}

use Act::{Button, Drag, Press, Release, Steer, Tap, Wait};
use KeyCode::{KeyA, KeyD, KeyE, KeyQ, KeyS, KeyW, NumpadDivide, Space};
use MouseButton::{Left, Right};

const PAST_THE_PILLAR_UP_THE_RAMP_AND_OFF_ITS_EDGE: &[Act] = &[
    Press(KeyW),
    Wait { secs: 1.0 },
    Press(KeyA),
    Wait { secs: 0.4 },
    Release(KeyA),
    Press(KeyD),
    Wait { secs: 0.4 },
    Release(KeyD),
    Steer {
        to: [0.0, -23.0],
        reach: 1.0,
        secs: 8.0,
    },
    Steer {
        to: [0.0, -35.0],
        reach: 1.0,
        secs: 4.0,
    },
    Steer {
        to: [0.0, -45.0],
        reach: 1.0,
        secs: 4.0,
    },
];

const BACK_INTO_THE_PLATFORM_AND_STRAFE: &[Act] = &[
    Release(KeyW),
    Press(KeyS),
    Wait { secs: 1.0 },
    Release(KeyS),
    Press(KeyQ),
    Wait { secs: 0.8 },
    Release(KeyQ),
    Press(KeyW),
    Press(KeyE),
    Wait { secs: 0.6 },
    Release(KeyE),
];

const WALKING_OVER_THE_KERB: &[Act] = &[
    Tap(NumpadDivide),
    Steer {
        to: [-12.0, -54.0],
        reach: 1.0,
        secs: 6.0,
    },
    Tap(NumpadDivide),
];

const TURNED_BY_THE_MOUSE_ALONG_THE_WALL: &[Act] = &[
    Button(Right, true),
    Drag {
        rate: -540.0,
        secs: 0.5,
    },
    Wait { secs: 3.0 },
    Button(Right, false),
];

const INTO_THE_TROUGH_WEDGED_AND_OUT: &[Act] = &[
    Steer {
        to: [5.9, -74.0],
        reach: 0.2,
        secs: 8.0,
    },
    Tap(Space),
    Steer {
        to: [12.0, -74.0],
        reach: 0.05,
        secs: 1.5,
    },
    Tap(Space),
    Steer {
        to: [20.0, -74.0],
        reach: 1.0,
        secs: 4.0,
    },
    Steer {
        to: [12.0, -77.0],
        reach: 0.1,
        secs: 4.0,
    },
    Tap(Space),
    Steer {
        to: [4.0, -77.0],
        reach: 1.0,
        secs: 4.0,
    },
];

const ONTO_THE_BLOCK_AND_OFF: &[Act] = &[
    Steer {
        to: [4.0, -60.0],
        reach: 1.0,
        secs: 6.0,
    },
    Steer {
        to: [20.0, -62.0],
        reach: 1.0,
        secs: 6.0,
    },
    Steer {
        to: [20.0, -60.4],
        reach: 0.5,
        secs: 4.0,
    },
    Tap(Space),
    Steer {
        to: [20.0, -46.0],
        reach: 1.0,
        secs: 4.0,
    },
];

const A_STANDING_JUMP_STEERED_ONCE: &[Act] = &[
    Release(KeyW),
    Wait { secs: 0.3 },
    Tap(Space),
    Wait { secs: 0.1 },
    Press(KeyQ),
    Wait { secs: 0.8 },
    Release(KeyQ),
];

const RUN_ON_BOTH_BUTTONS: &[Act] = &[
    Button(Left, true),
    Button(Right, true),
    Drag {
        rate: 940.0,
        secs: 0.5,
    },
    Press(KeyA),
    Wait { secs: 0.5 },
    Release(KeyA),
    Wait { secs: 1.0 },
    Button(Left, false),
    Button(Right, false),
];

const UP_THE_SLOPES_TO_A_STEEP_FACE_AND_BACK: &[Act] = &[
    Press(KeyW),
    Steer {
        to: [54.0, -44.0],
        reach: 1.0,
        secs: 5.0,
    },
    Release(KeyW),
    Press(KeyA),
    Wait { secs: 1.0 },
    Release(KeyA),
    Press(KeyW),
    Wait { secs: 2.0 },
];

const OVER_THE_BUMPS: &[Act] = &[
    Steer {
        to: [-30.0, -20.0],
        reach: 1.0,
        secs: 16.0,
    },
    Steer {
        to: [-40.0, -60.0],
        reach: 1.0,
        secs: 8.0,
    },
    Steer {
        to: [-30.0, 10.0],
        reach: 1.0,
        secs: 12.0,
    },
    Release(KeyW),
    Press(KeyS),
    Wait { secs: 0.5 },
    Release(KeyS),
];

const SCRIPT: &[&[Act]] = &[
    PAST_THE_PILLAR_UP_THE_RAMP_AND_OFF_ITS_EDGE,
    BACK_INTO_THE_PLATFORM_AND_STRAFE,
    WALKING_OVER_THE_KERB,
    TURNED_BY_THE_MOUSE_ALONG_THE_WALL,
    INTO_THE_TROUGH_WEDGED_AND_OUT,
    ONTO_THE_BLOCK_AND_OFF,
    A_STANDING_JUMP_STEERED_ONCE,
    RUN_ON_BOTH_BUTTONS,
    UP_THE_SLOPES_TO_A_STEEP_FACE_AND_BACK,
    OVER_THE_BUMPS,
];

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x100_0000_01b3;

struct PathHash(u64);

impl PathHash {
    fn record(&mut self, p: &Player) {
        let state = u32::from(p.steep_support)
            | u32::from(p.wedged) << 1
            | u32::from(p.airborne_since.is_some()) << 2
            | u32::from(p.last_step_airborne) << 3;
        let words = [
            p.pos.x.to_bits(),
            p.pos.y.to_bits(),
            p.pos.z.to_bits(),
            p.vel_y.to_bits(),
            p.horiz_vel.x.to_bits(),
            p.horiz_vel.y.to_bits(),
            p.horiz_vel.z.to_bits(),
            p.face_yaw.to_bits(),
            p.model_yaw.to_bits(),
            p.move_flags,
            state,
        ];
        for byte in words.iter().flat_map(|w| w.to_le_bytes()) {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
    }
}

struct Course {
    app: App,
    path: PathHash,
    aims: usize,
    nudged_aim: Option<usize>,
}

impl Course {
    fn new(nudged_aim: Option<usize>) -> Self {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            InputPlugin,
            AssetPlugin::default(),
            CollisionPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / f64::from(HZ),
        )))
        .insert_resource(Player {
            pos: START,
            settling: true,
            ..Player::default()
        })
        .insert_resource(PlayerCapsule(player_capsule()))
        .init_resource::<CameraControl>()
        .init_resource::<CameraOptions>()
        .add_systems(Startup, body::spawn_body)
        .add_systems(Update, controller::control);
        app.world_mut().spawn((
            world::world_camera(Transform::default()),
            CameraRig {
                yaw: 0.0,
                pitch: LOGIN_PITCH,
            },
        ));
        colliders::spawn(app.world_mut());
        app.finish();
        app.cleanup();
        let mut course = Self {
            app,
            path: PathHash(FNV_OFFSET),
            aims: 0,
            nudged_aim,
        };
        course.settle();
        course
    }

    fn settle(&mut self) {
        let answers =
            |c: WorldCollision<'_, '_>| c.ray_body(START + Vec3::Y, Dir3::NEG_Y, 2.0).is_some();
        for _ in 0..60 {
            self.app.update();
            let world = self.app.world_mut();
            if world.run_system_once(answers).expect("the system runs") {
                world.resource_mut::<Player>().settling = false;
                return;
            }
        }
        panic!("the course never answered");
    }

    fn frames(secs: f32) -> usize {
        (secs * HZ).round() as usize
    }

    fn frame(&mut self, mouse_rate: f32) {
        if mouse_rate != 0.0 {
            self.app.world_mut().write_message(MouseMotion {
                delta: Vec2::new(mouse_rate / HZ, 0.0),
            });
        }
        self.app.update();
        self.path.record(self.app.world().resource::<Player>());
    }

    fn key(&mut self, key_code: KeyCode, down: bool) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key: Key::Unidentified(NativeKey::Unidentified),
            state: pressed(down),
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    fn act(&mut self, act: Act) {
        match act {
            Press(key) => self.key(key, true),
            Release(key) => self.key(key, false),
            Button(button, down) => {
                self.app.world_mut().write_message(MouseButtonInput {
                    button,
                    state: pressed(down),
                    window: Entity::PLACEHOLDER,
                });
            }
            Tap(key) => {
                self.key(key, true);
                self.frame(0.0);
                self.key(key, false);
            }
            Wait { secs } => (0..Self::frames(secs)).for_each(|_| self.frame(0.0)),
            Drag { rate, secs } => (0..Self::frames(secs)).for_each(|_| self.frame(rate)),
            Steer {
                to: [x, z],
                reach,
                secs,
            } => {
                for _ in 0..Self::frames(secs) {
                    let feet = self.app.world().resource::<Player>().pos;
                    let (dx, dz) = (x - feet.x, z - feet.z);
                    if dx * dx + dz * dz < reach * reach {
                        return;
                    }
                    let mut yaw = (-dx).atan2(-dz);
                    if self.nudged_aim == Some(self.aims) {
                        yaw = yaw.next_up();
                    }
                    self.aims += 1;
                    self.app.world_mut().resource_mut::<Player>().face_yaw = yaw;
                    self.frame(0.0);
                }
            }
        }
    }
}

fn pressed(down: bool) -> ButtonState {
    if down {
        ButtonState::Pressed
    } else {
        ButtonState::Released
    }
}

fn walk(nudged_aim: Option<usize>) -> u64 {
    let mut course = Course::new(nudged_aim);
    for &act in SCRIPT.iter().copied().flatten() {
        course.act(act);
    }
    course.path.0
}

#[test]
fn the_course_walks_the_recorded_path() {
    let hash = walk(None);
    assert_eq!(
        hash, RECORDED_PATH,
        "the walk left the recorded path; if the mover changed on purpose, record {hash:#018x}"
    );
}

#[test]
fn an_aim_one_float_step_off_walks_another_path() {
    assert_ne!(
        walk(Some(0)),
        RECORDED_PATH,
        "the hash cannot see a float step"
    );
}
