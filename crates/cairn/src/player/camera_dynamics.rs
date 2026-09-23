//! The client's camera options and the smart pivot, follow terrain and head bob behind them.

use bevy::prelude::*;

use super::camera::{CAM_PITCH_LIMIT, FollowStyle, follow_cmd};
use super::camera_channel::{Arm, CHANNEL_EPS, SmoothChannel};
use super::flags::{
    BACKWARD, FALLING, FORWARD, STRAFE_LEFT, STRAFE_RIGHT, SWIMMING, TURN_LEFT, TURN_RIGHT,
};

/// The client's camera options, defaulting as the client does.
#[derive(Resource, Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct CameraOptions {
    pub pivot: bool,
    /// Radians of yaw per motion event above which a drag is not mostly vertical.
    pub pivot_dx_max: f32,
    pub pivot_dy_min: f32,
    /// The pitch bias's return rate, deg/s.
    pub target_smooth_speed: f32,
    pub terrain_tilt: bool,
    /// The ground channel's rate, deg/s.
    pub ground_smooth_speed: f32,
    /// The ground channel's duration bounds, s, each scaled by the row's factor.
    pub tilt_time_min: f32,
    pub tilt_time_max: f32,
    pub bobbing: bool,
    pub bob_lr_amplitude: f32,
    pub bob_ud_amplitude: f32,
    pub bob_frequency: f32,
    /// The bob's decay rate.
    pub bob_smooth_speed: f32,
    /// The boom stops on the waterline, and the pivot keeps its corridor above the water.
    pub water_collision: bool,
}

impl Default for CameraOptions {
    fn default() -> Self {
        Self {
            pivot: true,
            pivot_dx_max: 0.05,
            pivot_dy_min: 0.0,
            target_smooth_speed: 90.0,
            terrain_tilt: false,
            ground_smooth_speed: 7.5,
            tilt_time_min: 3.0,
            tilt_time_max: 10.0,
            bobbing: false,
            bob_lr_amplitude: 2.0,
            bob_ud_amplitude: 2.0,
            bob_frequency: 0.8,
            bob_smooth_speed: 0.8,
            water_collision: true,
        }
    }
}

const YARDS_PER_INCH: f32 = 1.0 / 36.0;
/// The speed the bob's rate is measured against, yd/s.
const BOB_SPEED_DIVISOR: f32 = 7.2;
const BOB_SPEED_CLAMP: (f32, f32) = (0.5, 1.5);
pub const BOB_FIRST_PERSON_DISTANCE: f32 = 1.0 / 6.0;
const PROBE_THROTTLE: f32 = 0.1;
/// Follow terrain's horizontal probe: reach, pull-back, lift and drop, yd.
pub const PROBE_REACH: f32 = 10.0 / 3.0;
pub const PROBE_BACKOFF: f32 = 5.0 / 18.0;
pub const PROBE_LIFT: f32 = 5.0 / 3.0;
pub const PROBE_DROP: f32 = 64.0 / 9.0;

#[derive(Clone, Copy, Debug)]
pub struct DynamicsInput {
    pub options: CameraOptions,
    pub smooth_style: FollowStyle,
    pub subject: SubjectState,
    pub nearclip: f32,
    /// The liquid surface over the body's feet as the last step left it, Bevy Y.
    pub surface_y: Option<f32>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SubjectState {
    pub last_move_flags: u32,
    /// The body's facing, in the camera's yaw convention.
    pub facing: f32,
    /// The body's current speed, yd/s.
    pub speed: f32,
    pub camera_command: u32,
}

impl SubjectState {
    pub fn translating(&self) -> bool {
        self.last_move_flags & (FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT) != 0
    }
}

/// Smart pivot: a camera the boom's sweep has pinned against geometry, looking level or up, turns
/// its view instead of swinging its arm. The drag goes into a pitch bias applied to the view but
/// not to the seat, and eases home once any condition drops.
pub struct SmartPivot {
    bias: SmoothChannel,
}

impl Default for SmartPivot {
    fn default() -> Self {
        Self {
            bias: SmoothChannel::angular(),
        }
    }
}

impl SmartPivot {
    pub fn bias(&self) -> f32 {
        self.bias.live()
    }

    /// Routes one motion event's pitch: `None` when it lives wholly in the bias, else the delta
    /// the ordinary pitch takes.
    pub fn route_pitch(
        &mut self,
        d_pitch: f32,
        d_yaw: f32,
        pitch: f32,
        subject: &SubjectState,
        clipped: bool,
        cfg: &CameraOptions,
    ) -> Option<f32> {
        let verdict = cfg.pivot && !subject.translating() && pitch >= 0.0 && clipped;
        let displaced = !self.bias.in_flight() && self.bias().abs() >= CHANNEL_EPS;
        let vertical_drag =
            verdict && d_pitch.abs() > cfg.pivot_dy_min && d_yaw.abs() < cfg.pivot_dx_max;
        if displaced || vertical_drag {
            let mut bias = self.bias() + d_pitch;
            if pitch > 0.0 {
                bias = bias.min(CAM_PITCH_LIMIT - pitch);
            }
            self.bias.snap(bias);
            if bias >= 0.0 && pitch > 0.0 {
                return None;
            }
        }
        self.release(cfg);
        Some(d_pitch)
    }

    pub fn advance(
        &mut self,
        pitch: f32,
        subject: &SubjectState,
        clipped: bool,
        cfg: &CameraOptions,
        dt: f32,
    ) {
        let holding = cfg.pivot && !subject.translating() && pitch >= 0.0 && clipped;
        if !holding && self.bias().abs() >= CHANNEL_EPS {
            self.release(cfg);
        } else if holding {
            let live = self.bias();
            self.bias.snap(live);
        }
        self.bias.advance(dt);
    }

    fn release(&mut self, cfg: &CameraOptions) {
        self.bias
            .arm(&Arm::at(0.0, cfg.target_smooth_speed.to_radians()));
    }
}

/// Follow terrain: the camera leans with the ground the body is walking onto.
pub struct TerrainTilt {
    ground: SmoothChannel,
    slope_pitch: f32,
    since_probe: f32,
    handed_off: bool,
}

impl Default for TerrainTilt {
    fn default() -> Self {
        Self {
            ground: SmoothChannel::angular(),
            slope_pitch: 0.0,
            since_probe: PROBE_THROTTLE,
            handed_off: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TiltState {
    Fall,
    Idle,
    Move,
    Strafe,
    Swim,
    Turn,
}

impl TiltState {
    fn of(subject: &SubjectState) -> Self {
        let f = subject.last_move_flags;
        if f & SWIMMING != 0 {
            Self::Swim
        } else if f & FALLING != 0 {
            Self::Fall
        } else if f & (FORWARD | BACKWARD) != 0 {
            Self::Move
        } else if f & (STRAFE_LEFT | STRAFE_RIGHT) != 0 {
            Self::Strafe
        } else if f & (TURN_LEFT | TURN_RIGHT) != 0 {
            Self::Turn
        } else {
            Self::Idle
        }
    }

    fn row(self, style: FollowStyle) -> TiltRow {
        let (absorb, factor) = match self {
            _ if style == FollowStyle::Never => (0.0, -1.0),
            Self::Fall => (1.0, 0.75),
            Self::Idle if style == FollowStyle::Always => (1.0, 1.0),
            Self::Idle => (0.0, -1.0),
            Self::Move | Self::Strafe | Self::Turn => (1.0, 1.0),
            Self::Swim => (0.0, 1.0),
        };
        TiltRow {
            absorb,
            delay: 0.0,
            factor,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct TiltRow {
    absorb: f32,
    delay: f32,
    /// Negative holds the channel where it is.
    factor: f32,
}

impl TerrainTilt {
    /// The ground's lean this frame.
    pub fn pitch(&self) -> f32 {
        if self.handed_off {
            0.0
        } else {
            self.ground.live()
        }
    }

    /// On the mouse-look edges the lean moves into the pitch and back out; returns what the pitch
    /// owes.
    pub fn hand_off(&mut self, look_turns_body: bool) -> f32 {
        if look_turns_body == self.handed_off {
            return 0.0;
        }
        self.handed_off = look_turns_body;
        let live = self.ground.live();
        if look_turns_body { live } else { -live }
    }

    pub fn slope_to_pitch(slope: f32) -> f32 {
        const STEPS: [(f32, f32); 5] = [
            (0.36, 20.0),
            (0.27, 15.0),
            (0.18, 10.0),
            (0.09, 5.0),
            (0.0, 0.0),
        ];
        let magnitude = STEPS
            .iter()
            .find(|(key, _)| slope.abs() >= *key)
            .map_or(0.0, |(_, deg)| *deg)
            .to_radians();
        if slope < 0.0 { -magnitude } else { magnitude }
    }

    pub fn advance(
        &mut self,
        probe: impl FnOnce() -> f32,
        enabled: bool,
        subject: &SubjectState,
        style: FollowStyle,
        cfg: &CameraOptions,
        dt: f32,
    ) -> f32 {
        self.since_probe += dt;
        if !enabled {
            self.slope_pitch = 0.0;
        } else if self.since_probe >= PROBE_THROTTLE {
            self.since_probe = 0.0;
            self.slope_pitch = Self::slope_to_pitch(probe());
        }
        let TiltRow {
            absorb,
            delay,
            factor,
        } = TiltState::of(subject).row(style);
        if factor < 0.0 {
            let live = self.ground.live();
            self.ground.snap(live);
        } else {
            self.ground.arm(&Arm {
                target: absorb * self.slope_pitch,
                delay,
                factor,
                rate: cfg.ground_smooth_speed.to_radians(),
                duration_bounds: Some((cfg.tilt_time_min * factor, cfg.tilt_time_max * factor)),
            });
        }
        self.ground.advance(dt)
    }
}

/// Head bob: a figure of eight of the eye in first person while moving; a session arms on the
/// command word's edges whatever the option says.
#[derive(Default)]
pub struct HeadBob {
    armed: bool,
    last_command: Option<u32>,
    since_transition: f32,
    offset: Vec3,
    ramp_from: Vec3,
    ramp_duration: f32,
}

impl HeadBob {
    pub fn offset(&self) -> Vec3 {
        self.offset
    }

    fn arms(command: u32) -> bool {
        use follow_cmd as c;
        const TRANSLATING: u32 =
            c::FORWARD | c::BACKWARD | c::STRAFE_LEFT | c::STRAFE_RIGHT | c::AUTORUN;
        command & TRANSLATING != 0
            || (command & c::RIGHT_MOUSE != 0
                && command & (c::LEFT_MOUSE | c::TURN_LEFT | c::TURN_RIGHT) != 0)
    }

    /// `zoom` is the wheel's distance: a camera squeezed against a wall is not in first person.
    pub fn advance(&mut self, zoom: f32, subject: &SubjectState, cfg: &CameraOptions, dt: f32) {
        self.since_transition += dt;
        let want = Self::arms(subject.camera_command);
        if self.last_command.replace(subject.camera_command) != Some(subject.camera_command)
            && want != self.armed
        {
            if want {
                self.armed = true;
            } else {
                self.armed = false;
                self.ramp_from = self.offset;
                let largest = self.offset.abs().max_element();
                self.ramp_duration = largest / cfg.bob_smooth_speed.max(f32::EPSILON);
            }
            self.since_transition = 0.0;
        }
        let eligible = zoom <= BOB_FIRST_PERSON_DISTANCE
            && cfg.bobbing
            && subject.last_move_flags & (SWIMMING | FALLING) == 0
            && self.armed;
        if eligible {
            let rate = cfg.bob_frequency
                * (subject.speed / BOB_SPEED_DIVISOR).clamp(BOB_SPEED_CLAMP.0, BOB_SPEED_CLAMP.1);
            let phase = std::f32::consts::TAU * rate * self.since_transition;
            let left = Quat::from_rotation_y(subject.facing) * Vec3::NEG_X;
            self.offset = left * (cfg.bob_lr_amplitude * YARDS_PER_INCH * phase.sin())
                + Vec3::Y * (cfg.bob_ud_amplitude * YARDS_PER_INCH * (2.0 * phase).sin());
        } else if (self.offset.x + self.offset.y + self.offset.z).abs() >= CHANNEL_EPS {
            let s = self.since_transition / self.ramp_duration.max(f32::EPSILON);
            self.offset = if s >= 1.0 {
                Vec3::ZERO
            } else {
                self.ramp_from * (1.0 - (1.0 - (std::f32::consts::PI * s).cos()) * 0.5)
            };
        } else {
            self.offset = Vec3::ZERO;
        }
    }
}

#[cfg(test)]
mod tests;
