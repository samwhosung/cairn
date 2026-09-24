//! The camera's return to behind the body, on the client's follow styles.

use bevy::math::ops;

use super::super::gait::wrap_pi;

/// The return's average rate, deg/s.
pub const FOLLOW_SPEED_DEFAULT: f32 = 180.0;
const FOLLOW_TIME_MIN: f32 = 0.1;
const FOLLOW_TIME_MAX: f32 = 2.0;
const FOLLOW_EPS: f32 = 1.0e-3;

/// Does the camera return to behind the body on its own?
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum FollowStyle {
    /// The orbit offset stays where the hand left it. Keyboard turns still carry the camera.
    Never,
    /// Returns only while the body is being driven: the client's default.
    #[default]
    Smart,
    /// Every input edge arms a return, standing still included.
    Always,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Arming {
    delay: f32,
    /// Zero cancels a return.
    factor: f32,
}

impl FollowStyle {
    fn row(self, state: FollowState) -> Arming {
        let factor = match (self, state) {
            (Self::Never, _) | (Self::Smart, FollowState::Idle | FollowState::Stop) => 0.0,
            _ => 1.0,
        };
        Arming { delay: 0.0, factor }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FollowState {
    Turn,
    Strafe,
    Move,
    /// A movement input was released this edge.
    Stop,
    Idle,
}

/// The camera's input command word.
pub mod follow_cmd {
    pub const RIGHT_MOUSE: u32 = 0x1;
    pub const LEFT_MOUSE: u32 = 0x2;
    pub const FORWARD: u32 = 0x10;
    pub const BACKWARD: u32 = 0x20;
    pub const STRAFE_LEFT: u32 = 0x40;
    pub const STRAFE_RIGHT: u32 = 0x80;
    pub const TURN_LEFT: u32 = 0x100;
    pub const TURN_RIGHT: u32 = 0x200;
    pub const AUTORUN: u32 = 0x1000;
    pub const MOVE_BITS: u32 = FORWARD | BACKWARD | AUTORUN;
    pub const STRAFE_BITS: u32 = STRAFE_LEFT | STRAFE_RIGHT;
    pub const TURN_BITS: u32 = TURN_LEFT | TURN_RIGHT;
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FollowConfig {
    pub style: FollowStyle,
    /// deg/s.
    pub yaw_speed: f32,
}

impl Default for FollowConfig {
    fn default() -> Self {
        Self {
            style: FollowStyle::default(),
            yaw_speed: FOLLOW_SPEED_DEFAULT,
        }
    }
}

pub struct FollowInput {
    pub cfg: FollowConfig,
    /// The body's facing, in the camera's yaw convention.
    pub face_yaw: f32,
    pub command: u32,
}

impl FollowInput {
    pub fn state(&self, stopping: bool) -> FollowState {
        use follow_cmd as c;
        let held = |bits: u32| self.command & bits != 0;
        if held(c::TURN_BITS) || held(c::RIGHT_MOUSE) {
            FollowState::Turn
        } else if held(c::STRAFE_BITS) {
            FollowState::Strafe
        } else if held(c::MOVE_BITS) {
            FollowState::Move
        } else if stopping {
            FollowState::Stop
        } else {
            FollowState::Idle
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct FollowArm {
    from: f32,
    to: f32,
    dur: f32,
    delay: f32,
    elapsed: f32,
    armed_with: Arming,
}

#[derive(Default)]
pub struct FollowRig {
    last_command: Option<u32>,
    arm: Option<FollowArm>,
}

impl FollowRig {
    /// Runs the return for a frame; the camera yaw it wants, if any.
    pub fn advance(
        &mut self,
        input: &FollowInput,
        cam_yaw: f32,
        dt: f32,
        look_held: bool,
    ) -> Option<f32> {
        let word = input.command;
        let previous = self.last_command.replace(word);
        if look_held {
            self.arm = None;
            return None;
        }
        if let Some(p) = previous.filter(|p| *p != word) {
            use follow_cmd as c;
            let stopping = (p & !word) & (c::MOVE_BITS | c::STRAFE_BITS | c::TURN_BITS) != 0;
            self.arm(input, cam_yaw, stopping);
        }
        let arm = self.arm.as_mut()?;
        arm.elapsed += dt;
        let t = arm.elapsed - arm.delay;
        if t < 0.0 {
            return None;
        }
        let s = t / arm.dur;
        let offset = if s >= 1.0 {
            let to = arm.to;
            self.arm = None;
            to
        } else {
            let e = (1.0 - ops::cos(std::f32::consts::PI * s)) * 0.5;
            arm.from + (arm.to - arm.from) * e
        };
        Some(wrap_pi(input.face_yaw + offset))
    }

    #[allow(clippy::float_cmp)]
    fn arm(&mut self, input: &FollowInput, cam_yaw: f32, stopping: bool) {
        let armed_with = input.cfg.style.row(input.state(stopping));
        let Arming { delay, factor } = armed_with;
        if factor == 0.0 {
            self.arm = None;
            return;
        }
        let to = 0.0;
        let from = wrap_pi(cam_yaw - input.face_yaw);
        let gap = (to - from).abs();
        if gap < FOLLOW_EPS {
            return;
        }
        if self.arm.is_some_and(|a| {
            a.to == to
                && (a.armed_with.delay - delay).abs() < FOLLOW_EPS
                && (a.armed_with.factor - factor).abs() < FOLLOW_EPS
        }) {
            return;
        }
        let rate = input.cfg.yaw_speed.to_radians().max(FOLLOW_EPS);
        self.arm = Some(FollowArm {
            from,
            to,
            dur: (gap / rate * factor).clamp(FOLLOW_TIME_MIN, FOLLOW_TIME_MAX),
            delay,
            elapsed: 0.0,
            armed_with,
        });
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 120.0;
    const OFFSET: f32 = std::f32::consts::FRAC_PI_2;

    fn cfg(style: FollowStyle) -> FollowConfig {
        FollowConfig {
            style,
            yaw_speed: FOLLOW_SPEED_DEFAULT,
        }
    }

    fn run(rig: &mut FollowRig, cfg: FollowConfig, word: u32, cam_yaw: f32, secs: f32) -> f32 {
        let mut yaw = cam_yaw;
        for _ in 0..((secs / DT).round() as i32).max(0) {
            let input = FollowInput {
                cfg,
                face_yaw: 0.0,
                command: word,
            };
            if let Some(y) = rig.advance(&input, yaw, DT, false) {
                yaw = y;
            }
        }
        yaw
    }

    #[test]
    fn the_auto_follow_is_armed_by_an_input_edge_and_eases_home() {
        let smart = cfg(FollowStyle::Smart);
        let mut rig = FollowRig::default();
        let parked = run(&mut rig, smart, 0, OFFSET, 1.0);
        assert_eq!(
            parked, OFFSET,
            "Smart standing still leaves the camera alone"
        );
        let half = run(&mut rig, smart, follow_cmd::FORWARD, parked, 0.25);
        assert!(half < OFFSET * 0.75 && half > OFFSET * 0.25, "{half}");
        let home = run(&mut rig, smart, follow_cmd::FORWARD, half, 0.3);
        assert!(home.abs() < 1.0e-4, "{home}");
        assert_eq!(
            run(&mut rig, smart, follow_cmd::FORWARD, home + 0.4, 1.0),
            home + 0.4
        );

        let mut rig = FollowRig::default();
        let held = run(&mut rig, smart, follow_cmd::FORWARD, 0.0, 0.1);
        assert_eq!(run(&mut rig, smart, 0, held + OFFSET, 0.5), held + OFFSET);

        let always = cfg(FollowStyle::Always);
        let mut rig = FollowRig::default();
        let held = run(&mut rig, always, follow_cmd::FORWARD, 0.0, 0.1);
        assert!(run(&mut rig, always, 0, held + OFFSET, 1.0).abs() < 1.0e-4);

        let never = cfg(FollowStyle::Never);
        let mut rig = FollowRig::default();
        let _ = run(&mut rig, never, 0, OFFSET, 0.1);
        assert_eq!(
            run(&mut rig, never, follow_cmd::FORWARD, OFFSET, 2.0),
            OFFSET
        );

        let mut rig = FollowRig::default();
        let small = 5.0_f32.to_radians();
        let _ = run(&mut rig, smart, 0, small, DT);
        let mid = run(&mut rig, smart, follow_cmd::FORWARD, small, 0.05);
        assert!(mid.abs() > 1.0e-4, "the 0.1 s floor: {mid}");
        assert!(run(&mut rig, smart, follow_cmd::FORWARD, mid, 0.06).abs() < 1.0e-4);
    }

    #[test]
    fn entering_a_drag_cancels_the_return_in_flight() {
        let smart = cfg(FollowStyle::Smart);
        let mut rig = FollowRig::default();
        let _ = run(&mut rig, smart, 0, OFFSET, DT);
        let mid = run(&mut rig, smart, follow_cmd::FORWARD, OFFSET, 0.1);
        assert!(mid < OFFSET && mid > 0.0);
        let dragging = follow_cmd::FORWARD | follow_cmd::LEFT_MOUSE;
        let input = FollowInput {
            cfg: smart,
            face_yaw: 0.0,
            command: dragging,
        };
        assert!(rig.advance(&input, mid, DT, true).is_none());
        assert_eq!(run(&mut rig, smart, dragging, mid, 1.0), mid);
        assert!(run(&mut rig, smart, follow_cmd::FORWARD, mid, 1.0).abs() < 1.0e-4);
    }

    #[test]
    fn the_follow_state_reads_the_camera_input_word() {
        use follow_cmd as c;
        let state = |command: u32, stopping: bool| {
            FollowInput {
                cfg: FollowConfig::default(),
                face_yaw: 0.0,
                command,
            }
            .state(stopping)
        };
        assert_eq!(state(0, false), FollowState::Idle);
        assert_eq!(state(0, true), FollowState::Stop);
        assert_eq!(state(c::RIGHT_MOUSE, false), FollowState::Turn);
        assert_eq!(
            state(c::RIGHT_MOUSE | c::TURN_LEFT, false),
            FollowState::Turn
        );
        assert_eq!(state(c::STRAFE_LEFT, false), FollowState::Strafe);
        assert_eq!(
            state(c::RIGHT_MOUSE | c::LEFT_MOUSE, false),
            FollowState::Turn
        );
        assert_eq!(state(c::LEFT_MOUSE, false), FollowState::Idle);
        assert_eq!(state(c::AUTORUN, false), FollowState::Move);
    }
}
