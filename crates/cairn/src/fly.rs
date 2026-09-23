use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};

use crate::view::Pose;

const LOOK_PER_PIXEL: f32 = 0.003;
const MAX_PITCH: f32 = 1.54;
const START_SPEED: f32 = 100.0;
const SPEED_PER_NOTCH: f32 = 1.1;
const PIXELS_PER_NOTCH: f32 = 20.0;
const BOOST: f32 = 5.0;

#[derive(Component)]
pub struct Fly {
    yaw: f32,
    pitch: f32,
    speed: f32,
}

impl Fly {
    pub fn new(pose: Pose) -> Self {
        Self {
            yaw: pose.heading,
            pitch: pose.pitch,
            speed: START_SPEED,
        }
    }

    pub fn look(&mut self, yaw: f32, pitch: f32) {
        self.yaw = yaw;
        self.pitch = pitch.clamp(-MAX_PITCH, MAX_PITCH);
    }

    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }
}

pub fn fly(
    time: Res<'_, Time>,
    keys: Res<'_, ButtonInput<KeyCode>>,
    buttons: Res<'_, ButtonInput<MouseButton>>,
    motion: Res<'_, AccumulatedMouseMotion>,
    scroll: Res<'_, AccumulatedMouseScroll>,
    mut cursor: Query<'_, '_, &mut CursorOptions, With<PrimaryWindow>>,
    mut camera: Query<'_, '_, (&mut Transform, &mut Fly)>,
) {
    let Ok((mut transform, mut fly)) = camera.single_mut() else {
        return;
    };
    let looking = buttons.any_pressed([MouseButton::Left, MouseButton::Right]);
    if let Ok(mut cursor) = cursor.single_mut() {
        let grab = if looking {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        if cursor.grab_mode != grab {
            cursor.grab_mode = grab;
            cursor.visible = !looking;
        }
    }
    if looking {
        fly.yaw -= motion.delta.x * LOOK_PER_PIXEL;
        fly.pitch = (fly.pitch - motion.delta.y * LOOK_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
    }
    transform.rotation = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0);

    let notches = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_NOTCH,
    };
    fly.speed = (fly.speed * SPEED_PER_NOTCH.powf(notches)).clamp(1.0, 2000.0);

    let mut direction = Vec3::ZERO;
    for (key, toward) in [
        (KeyCode::KeyW, *transform.forward()),
        (KeyCode::KeyS, *transform.back()),
        (KeyCode::KeyA, *transform.left()),
        (KeyCode::KeyD, *transform.right()),
        (KeyCode::Space, Vec3::Y),
        (KeyCode::KeyC, Vec3::NEG_Y),
    ] {
        if keys.pressed(key) {
            direction += toward;
        }
    }
    let boost = if keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]) {
        BOOST
    } else {
        1.0
    };
    transform.translation += direction.normalize_or_zero() * fly.speed * boost * time.delta_secs();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bevy::input::InputPlugin;
    use bevy::time::TimeUpdateStrategy;
    use world::coords::bevy_to_wow;

    use super::*;

    fn wow_offset_while_held(pose: Pose, key: KeyCode) -> Vec3 {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, InputPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                100,
            )))
            .add_systems(Update, fly.before(world::WorldSystems));
        let camera = app
            .world_mut()
            .spawn((pose.transform(), Fly::new(pose)))
            .id();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        for _ in 0..5 {
            app.update();
        }
        let at = app
            .world()
            .get::<Transform>(camera)
            .expect("the camera")
            .translation;
        Vec3::from_array(bevy_to_wow(at)) - pose.eye
    }

    #[test]
    fn w_flies_along_the_heading_and_space_rises() {
        let west = Pose::orbit(Vec3::ZERO, 90.0, 0.0, 10.0);
        let forward = wow_offset_while_held(west, KeyCode::KeyW);
        assert!(forward.y > 1.0 && forward.x.abs() < 1e-3 && forward.z.abs() < 1e-3);
        let up = wow_offset_while_held(west, KeyCode::Space);
        assert!(up.z > 1.0 && up.x.abs() < 1e-3 && up.y.abs() < 1e-3);
    }
}
