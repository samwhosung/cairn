//! Walking the world as the client walks it: the body, its movement and swimming, and the follow
//! camera.

mod body;
mod camera;
mod camera_channel;
mod camera_dynamics;
mod camera_water;
mod controller;
#[cfg(test)]
mod course;
#[cfg(test)]
mod fixture;
mod flags;
pub(crate) mod gait;
mod hearing;
mod input;
pub(crate) mod mover;
mod posture;
#[cfg(test)]
mod scenarios;
pub(crate) mod state;
pub(crate) mod swim;

use avian3d::prelude::Collider;
use bevy::prelude::*;
use world::coords::wow_to_bevy;
use world::unit::{CharacterLook, UnitAlpha, UnitMotion, UnitSystems};
use world::{WorldCamera, WorldSystems};

use crate::fly::{Fly, fly};
use crate::view::Pose;
pub use body::PlayerBody;
use body::PlayerLook;
pub(crate) use body::character_body;
pub(crate) use camera::CameraRig;
use camera::{CameraControl, LOGIN_PITCH};
use camera_dynamics::CameraOptions;
pub(crate) use state::Player;
use state::{CAPSULE_HEIGHT, CAPSULE_RADIUS};

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Walk,
    Fly,
}

#[derive(Resource)]
pub struct PlayerCapsule(pub Collider);

/// The body was put where it stands without walking there, and the server must be asked to put it
/// there too.
#[derive(Message)]
pub struct Teleported;

/// Walks a character of `look` standing at the pose's target, facing its heading, or flies the
/// pose's camera when `mode` is [`Mode::Fly`].
pub struct PlayerPlugin {
    pub pose: Pose,
    pub mode: Mode,
    pub look: CharacterLook,
}

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        let (pose, mode) = (self.pose, self.mode);
        app.insert_resource(mode)
            .insert_resource(PlayerLook(self.look.clone()))
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
            .add_message::<Teleported>()
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
            .add_systems(Startup, body::spawn_body)
            .add_systems(
                Update,
                (
                    body::dress_body,
                    body::pivot_on_model,
                    switch_mode,
                    controller::control.run_if(resource_equals(Mode::Walk)),
                    fly.run_if(resource_equals(Mode::Fly)),
                    hearing::publish_body,
                )
                    .chain()
                    .before(WorldSystems)
                    .before(UnitSystems),
            );
    }
}

fn switch_mode(
    keys: Res<'_, ButtonInput<KeyCode>>,
    mut mode: ResMut<'_, Mode>,
    mut player: ResMut<'_, Player>,
    mut teleported: MessageWriter<'_, Teleported>,
    mut camera: Query<'_, '_, (&Transform, &mut CameraRig, &mut Fly), With<WorldCamera>>,
    mut body: Query<'_, '_, (&mut UnitMotion, &mut UnitAlpha), With<PlayerBody>>,
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
            for (mut motion, mut alpha) in &mut body {
                *motion = UnitMotion::default();
                alpha.alpha = 1.0;
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
                teleported.write(Teleported);
            }
            *mode = Mode::Walk;
        }
        _ => {}
    }
}
