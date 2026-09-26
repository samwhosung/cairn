use std::f32::consts::FRAC_PI_4;

use bevy::camera::{CameraOutputMode, PerspectiveProjection, Projection};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponent;
use bevy::render::view::{Hdr, Msaa, NoIndirectDrawing};

/// How far the detailed world is drawn, in yards of view depth: the client's default `farclip`.
pub const FARCLIP: f32 = 350.0;
/// The client's default `nearclip`, in yards.
pub const NEARCLIP: f32 = 0.1;
/// The projection's far plane, well past [`FARCLIP`] so the horizon draws behind the wall.
pub const PROJECTION_FAR: f32 = 3000.0;
pub const FOV_Y: f32 = FRAC_PI_4;

pub(crate) fn nearest_depth_within_farclip(
    cam_pos: Vec3,
    cam_fwd: Vec3,
    center: Vec3,
    radius: f32,
) -> bool {
    (center - cam_pos).dot(cam_fwd) - radius <= FARCLIP
}

/// The camera the world is drawn through: the one lighting, fog and streaming follow.
#[derive(Component, Clone, Copy, ExtractComponent)]
pub struct WorldCamera;

/// A world camera at `transform`. Its frame holds gamma-space colour, as the client's framebuffer
/// does, without multisampling, like the client by default; the world's decode pass writes it to
/// the target. The camera draws a batch's instances in the order they were queued; drawn
/// indirectly, they would take their places in the order the GPU's threads reach them.
pub fn world_camera(transform: Transform) -> impl Bundle {
    (
        Camera3d::default(),
        WorldCamera,
        Camera {
            output_mode: CameraOutputMode::Skip,
            ..Camera::default()
        },
        Projection::from(PerspectiveProjection {
            fov: FOV_Y,
            near: NEARCLIP,
            far: PROJECTION_FAR,
            ..PerspectiveProjection::default()
        }),
        Hdr,
        Tonemapping::None,
        Msaa::Off,
        NoIndirectDrawing,
        transform,
    )
}
