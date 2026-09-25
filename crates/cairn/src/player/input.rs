//! The client's default movement bindings, read from Bevy's input, and the netted axes every
//! consumer reads instead of the keys.

use bevy::prelude::*;

use super::camera::{CameraControl, LookButton, follow_cmd};
use super::state::{Player, autorun_cancelled, forward_axis};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Binding {
    /// Both-button run through one binding: the middle mouse button.
    MoveAndSteer,
    MoveForward,
    MoveBackward,
    TurnLeft,
    TurnRight,
    StrafeLeft,
    StrafeRight,
    Jump,
    ToggleAutorun,
    ToggleRun,
    SitOrStand,
}

impl Binding {
    fn keys(self) -> &'static [KeyCode] {
        match self {
            Self::MoveAndSteer => &[],
            Self::MoveForward => &[KeyCode::KeyW, KeyCode::ArrowUp],
            Self::MoveBackward => &[KeyCode::KeyS, KeyCode::ArrowDown],
            Self::TurnLeft => &[KeyCode::KeyA, KeyCode::ArrowLeft],
            Self::TurnRight => &[KeyCode::KeyD, KeyCode::ArrowRight],
            Self::StrafeLeft => &[KeyCode::KeyQ],
            Self::StrafeRight => &[KeyCode::KeyE],
            Self::Jump => &[KeyCode::Space, KeyCode::Numpad0],
            Self::ToggleAutorun => &[KeyCode::NumLock],
            Self::ToggleRun => &[KeyCode::NumpadDivide],
            Self::SitOrStand => &[KeyCode::KeyX],
        }
    }

    fn buttons(self) -> &'static [MouseButton] {
        match self {
            Self::MoveAndSteer => &[MouseButton::Middle],
            Self::ToggleAutorun => &[MouseButton::Forward],
            _ => &[],
        }
    }
}

#[derive(Clone, Copy)]
pub struct Keys<'a> {
    pub keys: &'a ButtonInput<KeyCode>,
    pub mouse: &'a ButtonInput<MouseButton>,
}

impl Keys<'_> {
    pub fn held(&self, c: Binding) -> bool {
        self.keys.any_pressed(c.keys().iter().copied())
            || self.mouse.any_pressed(c.buttons().iter().copied())
    }

    pub fn pressed_now(&self, c: Binding) -> bool {
        self.keys.any_just_pressed(c.keys().iter().copied())
            || self.mouse.any_just_pressed(c.buttons().iter().copied())
    }
}

pub struct LookInput {
    pub both_buttons: bool,
    pub camera_command: u32,
}

pub fn look_input(keys: Keys<'_>, player: &Player, rig: &CameraControl) -> LookInput {
    use Binding as C;
    use follow_cmd as bit;
    let steer = keys.held(C::MoveAndSteer);
    let mut w = 0;
    for (on, b) in [
        (
            rig.world_mouse.held(LookButton::Right) || steer,
            bit::RIGHT_MOUSE,
        ),
        (
            rig.world_mouse.held(LookButton::Left) || steer,
            bit::LEFT_MOUSE,
        ),
        (keys.held(C::MoveForward), bit::FORWARD),
        (keys.held(C::MoveBackward), bit::BACKWARD),
        (keys.held(C::StrafeLeft), bit::STRAFE_LEFT),
        (keys.held(C::StrafeRight), bit::STRAFE_RIGHT),
        (keys.held(C::TurnLeft), bit::TURN_LEFT),
        (keys.held(C::TurnRight), bit::TURN_RIGHT),
        (player.autorun, bit::AUTORUN),
    ] {
        if on {
            w |= b;
        }
    }
    LookInput {
        both_buttons: rig.world_mouse.both() || steer,
        camera_command: w,
    }
}

#[derive(Clone, Copy, Default, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct MoveAxes {
    /// `+1` forward, `−1` back, `0` for a cancelled pair.
    pub fwd: i32,
    /// `+` right: Q/E always, A/D while mouse-looking.
    pub side: i32,
    /// Right mouse or both buttons: A/D strafe and the facing follows the camera.
    pub mouselook: bool,
    /// A keyboard turn is held.
    pub turning: bool,
    /// Off the net axes: W+S moves nothing.
    pub translating: bool,
    pub turn_left: bool,
    pub turn_right: bool,
}

/// Decodes the movement keys, running the autorun and walk toggles on the way.
pub fn move_axes(
    keys: Keys<'_>,
    player: &mut Player,
    rig: &CameraControl,
    both_buttons: bool,
) -> MoveAxes {
    use Binding as C;
    if keys.pressed_now(C::ToggleAutorun) {
        player.autorun = !player.autorun;
    }
    if keys.pressed_now(C::ToggleRun) {
        player.walking = !player.walking;
    }
    let both_engaged = (both_buttons
        && (rig.world_mouse.down(LookButton::Left) || rig.world_mouse.down(LookButton::Right)))
        || keys.pressed_now(C::MoveAndSteer);
    if autorun_cancelled(
        keys.pressed_now(C::MoveForward),
        keys.pressed_now(C::MoveBackward),
        both_engaged,
    ) {
        player.autorun = false;
    }
    let fwd = forward_axis(
        keys.held(C::MoveForward),
        keys.held(C::MoveBackward),
        both_buttons,
        player.autorun,
    );
    let mouselook = both_buttons || rig.look == Some(LookButton::Right);
    let (strafe_left, strafe_right) = (keys.held(C::StrafeLeft), keys.held(C::StrafeRight));
    let (turn_left, turn_right) = (keys.held(C::TurnLeft), keys.held(C::TurnRight));
    let side = i32::from(strafe_right) - i32::from(strafe_left)
        + if mouselook {
            i32::from(turn_right) - i32::from(turn_left)
        } else {
            0
        };
    MoveAxes {
        fwd,
        side,
        mouselook,
        turning: !mouselook && (turn_left || turn_right),
        translating: fwd != 0 || side != 0,
        turn_left,
        turn_right,
    }
}
