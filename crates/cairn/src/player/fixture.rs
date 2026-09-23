//! Headless worlds for the mover's and the camera's tests.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::transform::TransformPlugin;

use super::state::{CAPSULE_HEIGHT, CAPSULE_RADIUS};

/// A headless app with the client's physics set, stepping every update.
pub fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        world::collision::physics_plugins(PostUpdate),
    ));
    app.finish();
    app.cleanup();
    app
}

/// A `(x, y)` polyline extruded across `z ∈ [−3, 3]`, every face wound up and back toward a body
/// approaching along +x.
pub fn world_from_profile(profile: &[(f32, f32)]) -> App {
    const W: f32 = 3.0;
    let mut app = physics_app();
    let (mut verts, mut tris) = (Vec::new(), Vec::new());
    for w in profile.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        let b = verts.len() as u32;
        verts.extend([
            Vec3::new(x0, y0, -W),
            Vec3::new(x1, y1, -W),
            Vec3::new(x1, y1, W),
            Vec3::new(x0, y0, W),
        ]);
        tris.extend([[b, b + 2, b + 1], [b, b + 3, b + 2]]);
    }
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::trimesh(verts, tris),
        Transform::default(),
    ));
    app.update();
    app
}

pub fn player_capsule() -> Collider {
    Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT - 2.0 * CAPSULE_RADIUS)
}

/// One frame's clock at `hz`.
pub fn frame_time(hz: f32) -> Time {
    let mut time = Time::default();
    time.advance_by(std::time::Duration::from_secs_f32(1.0 / hz));
    time
}
