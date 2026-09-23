//! Walking the world as the client walks it: the body, its movement and swimming, and the follow
//! camera.

mod camera;
mod camera_channel;
mod camera_dynamics;
mod camera_water;
mod controller;
#[cfg(test)]
mod fixture;
mod flags;
mod gait;
mod input;
mod mover;
mod state;
mod swim;

use avian3d::prelude::Collider;
use bevy::prelude::*;
use world::coords::wow_to_bevy;
use world::{WorldCamera, WorldSystems};

use crate::fly::{Fly, fly};
use crate::view::Pose;
use camera::{CameraControl, CameraRig, LOGIN_PITCH};
use camera_dynamics::CameraOptions;
use state::{CAPSULE_HEIGHT, CAPSULE_RADIUS, Player};

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Walk,
    Fly,
}

#[derive(Resource)]
pub struct PlayerCapsule(pub Collider);

#[derive(Component)]
pub struct StandIn;

/// Walks a body standing at the pose's target, facing its heading, or flies the pose's camera
/// when `mode` is [`Mode::Fly`].
pub struct PlayerPlugin {
    pub pose: Pose,
    pub mode: Mode,
}

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        let (pose, mode) = (self.pose, self.mode);
        app.insert_resource(mode)
            .insert_resource(Player {
                pos: wow_to_bevy(pose.target.to_array()),
                face_yaw: pose.heading,
                model_yaw: pose.heading,
                settling: true,
                ..Player::default()
            })
            .insert_resource(PlayerCapsule(Collider::capsule(
                CAPSULE_RADIUS,
                CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
            )))
            .init_resource::<CameraControl>()
            .init_resource::<CameraOptions>()
            .add_systems(Startup, move |mut commands: Commands<'_, '_>| {
                commands.spawn((
                    world::world_camera(pose.transform()),
                    CameraRig {
                        yaw: pose.heading,
                        pitch: LOGIN_PITCH,
                    },
                    Fly::new(pose),
                ));
            })
            .add_systems(Startup, spawn_stand_in)
            .add_systems(
                Update,
                (
                    switch_mode,
                    controller::control.run_if(resource_equals(Mode::Walk)),
                    fly.run_if(resource_equals(Mode::Fly)),
                )
                    .chain()
                    .before(WorldSystems),
            );
    }
}

fn spawn_stand_in(
    mut commands: Commands<'_, '_>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<StandardMaterial>>,
) {
    commands.spawn((
        StandIn,
        Mesh3d(meshes.add(Capsule3d::new(
            CAPSULE_RADIUS,
            CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS,
        ))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.6, 0.7),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..StandardMaterial::default()
        })),
        Transform::default(),
    ));
}

fn switch_mode(
    keys: Res<'_, ButtonInput<KeyCode>>,
    mut mode: ResMut<'_, Mode>,
    mut player: ResMut<'_, Player>,
    mut camera: Query<'_, '_, (&Transform, &mut CameraRig, &mut Fly), With<WorldCamera>>,
    body: Query<'_, '_, &MeshMaterial3d<StandardMaterial>, With<StandIn>>,
    mut materials: ResMut<'_, Assets<StandardMaterial>>,
) {
    let chord = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight])
        && keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let Ok((transform, mut rig, mut fly)) = camera.single_mut() else {
        return;
    };
    let toggled = chord && keys.just_pressed(KeyCode::KeyF);
    let landed = chord && keys.just_pressed(KeyCode::KeyG) && *mode == Mode::Fly;
    match *mode {
        Mode::Walk if toggled => {
            let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);
            fly.look(yaw, pitch);
            for material in &body {
                if let Some(m) = materials.get_mut(&material.0) {
                    m.base_color.set_alpha(1.0);
                }
            }
            *mode = Mode::Fly;
        }
        Mode::Fly if toggled || landed => {
            rig.yaw = fly.yaw();
            rig.pitch = fly.pitch();
            if landed {
                player.pos = transform.translation;
                player.face_yaw = fly.yaw();
                player.vel_y = 0.0;
                player.settling = true;
            }
            *mode = Mode::Walk;
        }
        _ => {}
    }
}
