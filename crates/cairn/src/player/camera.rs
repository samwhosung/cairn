//! The follow camera: mouse look, wheel zoom and the collided boom.

mod follow;
mod pivot;

use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions};
use world::collision::WorldCollision;

pub use follow::{FollowConfig, FollowInput, FollowRig, FollowStyle, follow_cmd};
pub use pivot::{
    CameraPivot, PivotGlide, SELF_FADE_WINDOW, model_pivot_height, self_model_fade_alpha,
};

use super::camera_dynamics::{
    DynamicsInput, HeadBob, PROBE_BACKOFF, PROBE_DROP, PROBE_LIFT, PROBE_REACH, SmartPivot,
    TerrainTilt,
};
use super::camera_water;

/// The zoom's range, yd: first person to the default ceiling.
pub const CAM_DIST_MIN: f32 = 0.0;
pub const CAM_DIST_MAX: f32 = 15.0;
pub const CAM_DIST_DEFAULT: f32 = 15.0;
/// One wheel notch, yd.
const CAM_ZOOM_STEP: f32 = 1.0;
/// The zoom glides to the wheel's target at this constant speed, yd/s.
const CAM_MOVE_SPEED: f32 = 8.33;
/// Radians of camera turn per unit of mouse motion at the default speeds.
const LOOK_SENSITIVITY: f32 = 0.003;
pub const CAM_PITCH_LIMIT: f32 = 89.0 * std::f32::consts::PI / 180.0;
/// How fast the boom eases back out once an obstruction clears, 1/s.
const CAM_RETURN_RATE: f32 = 6.0;
pub const LOGIN_PITCH: f32 = -0.45;

/// The mouse-look rates: the client's per-axis law at its default speeds, scaled to raw mouse
/// motion.
#[derive(Clone, Copy, Debug)]
pub struct LookConfig {
    pub invert_pitch: bool,
    pub sensitivity: f32,
    pub yaw_speed: f32,
    pub pitch_speed: f32,
}

impl Default for LookConfig {
    fn default() -> Self {
        Self {
            invert_pitch: false,
            sensitivity: 1.0,
            yaw_speed: 180.0,
            pitch_speed: 90.0,
        }
    }
}

impl LookConfig {
    pub fn yaw_rate(self) -> f32 {
        self.yaw_speed * (LOOK_SENSITIVITY / 180.0) * self.sensitivity
    }

    pub fn pitch_rate(self) -> f32 {
        self.pitch_speed * (LOOK_SENSITIVITY / 90.0) * self.sensitivity
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LookButton {
    Right,
    Left,
}

impl LookButton {
    fn button(self) -> MouseButton {
        match self {
            Self::Right => MouseButton::Right,
            Self::Left => MouseButton::Left,
        }
    }
}

/// The mouse buttons the world holds: claimed on the press, kept until that button's release.
#[derive(Default, Clone, Copy, Debug)]
pub struct WorldMouse {
    held: [bool; 2],
    down: [bool; 2],
}

impl WorldMouse {
    pub fn held(self, b: LookButton) -> bool {
        self.held[b as usize]
    }

    pub fn down(self, b: LookButton) -> bool {
        self.down[b as usize]
    }

    pub fn both(self) -> bool {
        self.held(LookButton::Right) && self.held(LookButton::Left)
    }

    /// `world_press` says whether a press this frame belongs to the world.
    pub fn update(&mut self, buttons: &ButtonInput<MouseButton>, world_press: bool) {
        for b in [LookButton::Right, LookButton::Left] {
            let i = b as usize;
            self.down[i] = world_press && buttons.just_pressed(b.button());
            self.held[i] = (self.held[i] || self.down[i]) && buttons.pressed(b.button());
        }
    }
}

/// The camera's yaw and pitch, in Bevy's convention: yaw about +Y, pitch positive up.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct CameraRig {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Resource)]
pub struct CameraControl {
    pub distance: f32,
    pub target_distance: f32,
    pub boom_length: f32,
    pub look: Option<LookButton>,
    pub look_turns_body: bool,
    pub world_mouse: WorldMouse,
    cursor_stash: Option<Vec2>,
    pub self_fade_alpha: f32,
    pub follow: FollowRig,
    pub pivot: PivotGlide,
    pub smart_pivot: SmartPivot,
    pub terrain_tilt: TerrainTilt,
    pub head_bob: HeadBob,
    pub boom_blocked: bool,
    pub look_config: LookConfig,
    pub follow_config: FollowConfig,
}

impl Default for CameraControl {
    fn default() -> Self {
        Self {
            distance: CAM_DIST_DEFAULT,
            target_distance: CAM_DIST_DEFAULT,
            boom_length: CAM_DIST_DEFAULT,
            look: None,
            look_turns_body: false,
            world_mouse: WorldMouse::default(),
            cursor_stash: None,
            self_fade_alpha: 1.0,
            follow: FollowRig::default(),
            pivot: PivotGlide::default(),
            smart_pivot: SmartPivot::default(),
            terrain_tilt: TerrainTilt::default(),
            head_bob: HeadBob::default(),
            boom_blocked: false,
            look_config: LookConfig::default(),
            follow_config: FollowConfig::default(),
        }
    }
}

/// The look session: a press of either button engages its look at once, the other button joins
/// or takes over, and the cursor hides and locks while a button is held. Motion turns the camera;
/// a right-drag or the both-button run turns the body with it.
#[allow(clippy::too_many_arguments)]
pub fn run_look_session(
    buttons: &ButtonInput<MouseButton>,
    motion: Vec2,
    both_buttons: bool,
    rig: &mut CameraControl,
    cam: &mut CameraRig,
    face_yaw: &mut f32,
    window: Option<(&mut Window, &mut CursorOptions)>,
    dynamics: &DynamicsInput,
) {
    let (mut window, mut cursor) = match window {
        Some((w, c)) => (Some(w), Some(c)),
        None => (None, None),
    };
    if let Some(active) = rig.look {
        if !buttons.pressed(active.button()) {
            let other = match active {
                LookButton::Right => LookButton::Left,
                LookButton::Left => LookButton::Right,
            };
            if rig.world_mouse.held(other) {
                rig.look = Some(other);
            } else {
                rig.look = None;
                if let Some(c) = cursor.as_deref_mut() {
                    c.grab_mode = CursorGrabMode::None;
                    c.visible = true;
                }
                if let (Some(w), Some(pos)) = (window.as_deref_mut(), rig.cursor_stash.take()) {
                    w.set_cursor_position(Some(pos));
                }
            }
        }
    } else {
        let engaged = if rig.world_mouse.down(LookButton::Right) {
            Some(LookButton::Right)
        } else if rig.world_mouse.down(LookButton::Left) {
            Some(LookButton::Left)
        } else {
            None
        };
        if let Some(b) = engaged {
            rig.look = Some(b);
            rig.cursor_stash = window.as_deref().and_then(Window::cursor_position);
            if let Some(c) = cursor {
                c.grab_mode = CursorGrabMode::Locked;
                c.visible = false;
            }
        }
    }
    if let Some(active) = rig.look {
        let (yaw_rate, pitch_rate) = (rig.look_config.yaw_rate(), rig.look_config.pitch_rate());
        let d_yaw = -motion.x * yaw_rate;
        cam.yaw += d_yaw;
        let dy = if rig.look_config.invert_pitch {
            -motion.y
        } else {
            motion.y
        };
        let d_pitch = -dy * pitch_rate;
        if let Some(d_pitch) = rig.smart_pivot.route_pitch(
            d_pitch,
            d_yaw,
            cam.pitch,
            &dynamics.subject,
            rig.boom_blocked,
            &dynamics.options,
        ) {
            cam.pitch = (cam.pitch + d_pitch).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
        }
        if active == LookButton::Right || both_buttons {
            *face_yaw = cam.yaw;
        }
    }
    rig.look_turns_body =
        rig.look == Some(LookButton::Right) || (rig.look.is_some() && both_buttons);
}

/// Positive `notches` zoom in.
pub fn apply_zoom_scroll(notches: f32, dt: f32, rig: &mut CameraControl) {
    if notches != 0.0 {
        rig.target_distance =
            (rig.target_distance - notches * CAM_ZOOM_STEP).clamp(CAM_DIST_MIN, CAM_DIST_MAX);
    }
    let max_step = CAM_MOVE_SPEED * dt;
    rig.distance += (rig.target_distance - rig.distance).clamp(-max_step, max_step);
}

pub struct Subject {
    /// Feet, Bevy space.
    pub feet: Vec3,
    /// Where the boom's sweep starts: the head, which the body's own collision keeps inside the
    /// room.
    pub head: Vec3,
    /// The pivot's target height above the feet; `None` holds the channel.
    pub pivot_target: Option<f32>,
    /// This frame's keyboard turn, which carries the camera rigidly.
    pub turn_delta: f32,
}

/// Seats the camera behind the subject.
#[allow(clippy::too_many_arguments)]
pub fn seat_camera(
    dt: f32,
    subject: &Subject,
    rig: &mut CameraControl,
    cam: &mut CameraRig,
    transform: &mut Transform,
    collide: &WorldCollision<'_, '_>,
    follow: &FollowInput,
    dynamics: &DynamicsInput,
) {
    let live_pivot = rig.pivot.advance(subject.pivot_target, dt);
    let pivot_target = rig.pivot.target();
    let (band, depth) = camera_water::classify(dynamics.surface_y, subject.feet.y, pivot_target);
    let corridor = if dynamics.options.water_collision {
        camera_water::corridor(band, depth, live_pivot)
    } else {
        camera_water::dry_corridor(live_pivot)
    };
    let pivot_height = camera_water::pivot_height(corridor, pivot_target, live_pivot);
    let sweep_from = subject
        .head
        .with_y(subject.head.y.max(subject.feet.y + corridor.floor));

    let feet = subject.feet;
    let ground_probe = || {
        let origin = feet + Vec3::Y * PROBE_LIFT;
        let fwd = Quat::from_rotation_y(dynamics.subject.facing) * Vec3::NEG_Z;
        let reach = Dir3::new(fwd)
            .ok()
            .and_then(|d| collide.ray_body(origin, d, PROBE_REACH))
            .map_or(PROBE_REACH, |h| h.distance - PROBE_BACKOFF);
        let ahead = origin + fwd * reach;
        let ground_y = collide
            .ray_body(ahead, Dir3::NEG_Y, PROBE_DROP)
            .map_or(ahead.y - PROBE_DROP, |h| ahead.y - h.distance);
        (ground_y - feet.y) / reach.abs().max(1.0e-3)
    };
    rig.terrain_tilt.advance(
        ground_probe,
        dynamics.options.terrain_tilt,
        &dynamics.subject,
        dynamics.smooth_style,
        &dynamics.options,
        dt,
    );
    let handed = rig.terrain_tilt.hand_off(rig.look_turns_body);
    if handed != 0.0 {
        cam.pitch = (cam.pitch + handed).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    }
    rig.head_bob
        .advance(rig.distance, &dynamics.subject, &dynamics.options, dt);

    let look_held = rig.look.is_some();
    if !look_held {
        cam.yaw += subject.turn_delta;
    }
    if let Some(yaw) = rig.follow.advance(follow, cam.yaw, dt, look_held) {
        cam.yaw = yaw;
    }
    let bias = rig.smart_pivot.bias();
    let arm_pitch = (cam.pitch + rig.terrain_tilt.pitch()).clamp(-CAM_PITCH_LIMIT, CAM_PITCH_LIMIT);
    let arm_rotation = Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch, 0.0);
    let view_rotation = if bias == 0.0 {
        arm_rotation
    } else {
        Quat::from_euler(EulerRot::YXZ, cam.yaw, arm_pitch + bias, 0.0)
    };
    let pivot = feet + Vec3::Y * pivot_height;
    let seat = pivot - arm_rotation * Vec3::NEG_Z * rig.distance;
    let boom = seat - sweep_from;
    let boom_len = boom.length().max(1.0e-3);
    let hit = collide.cast_camera(sweep_from, boom, dynamics.options.water_collision);
    rig.boom_blocked = hit.is_some();
    let open = hit.unwrap_or(boom_len);
    rig.boom_length = if open < rig.boom_length {
        open
    } else {
        let t = 1.0 - (-CAM_RETURN_RATE * dt).exp();
        rig.boom_length + (open - rig.boom_length) * t
    };
    let seated = sweep_from + boom * (rig.boom_length / boom_len).clamp(0.0, 1.0);
    let translation = seated + rig.head_bob.offset();
    if transform.rotation != view_rotation || transform.translation != translation {
        transform.rotation = view_rotation;
        transform.translation = translation;
    }
    rig.smart_pivot.advance(
        cam.pitch,
        &dynamics.subject,
        rig.boom_blocked,
        &dynamics.options,
        dt,
    );
    rig.self_fade_alpha = self_model_fade_alpha(
        (seated - pivot).length(),
        dynamics.nearclip,
        SELF_FADE_WINDOW,
    );
}

#[cfg(test)]
mod tests;
