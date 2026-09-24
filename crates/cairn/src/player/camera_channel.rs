//! The camera's smoothed scalar: one template the client uses for the pitch bias, the ground tilt
//! and the pivot height.

use bevy::math::ops;

pub const CHANNEL_EPS: f32 = 0.001;

/// `rate` is in the live value's units per second.
#[derive(Clone, Copy, Debug)]
pub struct Arm {
    pub target: f32,
    /// Dead time before the tween, s; held at the start value meanwhile.
    pub delay: f32,
    pub factor: f32,
    pub rate: f32,
    pub duration_bounds: Option<(f32, f32)>,
}

impl Arm {
    pub fn at(target: f32, rate: f32) -> Self {
        Self {
            target,
            delay: 0.0,
            factor: 1.0,
            rate,
            duration_bounds: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Flight {
    elapsed: f32,
    delay: f32,
    /// Seconds.
    duration: f32,
    memo: (f32, f32),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SmoothChannel {
    live: f32,
    from: f32,
    to: f32,
    flight: Option<Flight>,
    angular: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Armed {
    Started,
    Already,
    AtRest,
}

impl SmoothChannel {
    /// A channel whose live value is an angle in radians.
    pub fn angular() -> Self {
        Self {
            angular: true,
            ..Self::default()
        }
    }

    pub fn live(&self) -> f32 {
        self.live
    }

    pub fn in_flight(&self) -> bool {
        self.flight.is_some()
    }

    pub fn snap(&mut self, v: f32) {
        self.live = v;
        self.from = v;
        self.to = v;
        self.flight = None;
    }

    pub fn arm(&mut self, arm: &Arm) -> Armed {
        if self.angular {
            use std::f32::consts::{PI, TAU};
            while self.live - arm.target > PI {
                self.live -= TAU;
            }
            while arm.target - self.live > PI {
                self.live += TAU;
            }
        }
        if let Some(f) = self.flight
            && (self.to - arm.target).abs() < CHANNEL_EPS
            && (f.memo.0 - arm.delay).abs() < CHANNEL_EPS
            && (f.memo.1 - arm.factor).abs() < CHANNEL_EPS
        {
            return Armed::Already;
        }
        let gap = (arm.target - self.live).abs();
        if gap < CHANNEL_EPS {
            self.to = arm.target;
            self.flight = None;
            return Armed::AtRest;
        }
        let mut duration = gap / arm.rate.max(f32::EPSILON) * arm.factor;
        if let Some((lo, hi)) = arm.duration_bounds {
            duration = duration.clamp(lo, hi);
        }
        self.from = self.live;
        self.to = arm.target;
        self.flight = Some(Flight {
            elapsed: 0.0,
            delay: arm.delay,
            duration: duration.max(f32::EPSILON),
            memo: (arm.delay, arm.factor),
        });
        Armed::Started
    }

    pub fn advance(&mut self, dt: f32) -> f32 {
        if let Some(f) = self.flight.as_mut() {
            f.elapsed += dt;
            let t = f.elapsed - f.delay;
            if t >= 0.0 {
                let s = t / f.duration;
                if s >= 1.0 {
                    self.live = self.to;
                    self.flight = None;
                } else {
                    let e = (1.0 - ops::cos(std::f32::consts::PI * s)) * 0.5;
                    self.live = self.from + (self.to - self.from) * e;
                }
            }
        }
        self.live
    }

    pub fn target(&self) -> f32 {
        self.to
    }
}

/// Walks `f` across `[from, to]` and asserts no step moves its output more than `max_jump`.
#[cfg(test)]
pub fn assert_bounded_step(
    (from, to): (f32, f32),
    step: f32,
    max_jump: f32,
    mut f: impl FnMut(f32) -> f32,
) {
    let mut previous: Option<(f32, f32)> = None;
    let steps = ((to - from) / step).ceil() as i32;
    for i in 0..=steps {
        let x = (from + step * i as f32).min(to);
        let y = f(x);
        if let Some((px, py)) = previous {
            assert!(
                (y - py).abs() <= max_jump,
                "{px} to {x} moved {py} -> {y}, past {max_jump}"
            );
        }
        previous = Some((x, y));
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn run(c: &mut SmoothChannel, secs: f32) -> Vec<f32> {
        let dt = 1.0 / 60.0;
        (0..(secs / dt).round() as usize)
            .map(|_| c.advance(dt))
            .collect()
    }

    #[test]
    fn the_duration_is_the_gap_over_the_rate_times_the_factor() {
        for (gap, rate, factor) in [
            (10.0_f32, 45.0_f32, 1.0_f32),
            (30.0, 90.0, 2.0),
            (1.0, 7.5, 1.0),
        ] {
            let mut c = SmoothChannel::default();
            let expected = gap / rate * factor;
            let arm = Arm {
                factor,
                ..Arm::at(gap, rate)
            };
            assert_eq!(c.arm(&arm), Armed::Started);
            let frames = run(&mut c, expected * 2.0);
            let arrived = frames
                .iter()
                .position(|v| (v - gap).abs() < CHANNEL_EPS)
                .expect("arrives");
            assert!((arrived as f32 / 60.0 - expected).abs() < 0.05);
            assert!(!c.in_flight());
        }
    }

    #[test]
    fn a_per_frame_re_arm_neither_restarts_nor_stretches_the_tween() {
        let (mut once, mut every) = (SmoothChannel::default(), SmoothChannel::default());
        let arm = Arm::at(1.0, 0.5);
        once.arm(&arm);
        every.arm(&arm);
        for _ in 0..60 {
            assert_eq!(every.arm(&arm), Armed::Already);
            assert_eq!(every.advance(1.0 / 60.0), once.advance(1.0 / 60.0));
        }
        run(&mut every, 1.5);
        assert_eq!(every.arm(&arm), Armed::AtRest);
    }

    #[test]
    fn the_arm_rewraps_an_angle_to_the_short_side() {
        let mut c = SmoothChannel::angular();
        c.snap(3.1);
        c.arm(&Arm::at(-3.1, 1.0));
        let frames = run(&mut c, 1.0);
        assert!((frames[frames.len() - 1] + 3.1).abs() < CHANNEL_EPS);
        let wrapped = 3.1 - std::f32::consts::TAU;
        assert!(
            frames
                .iter()
                .all(|v| *v >= wrapped - CHANNEL_EPS && *v <= -3.1 + CHANNEL_EPS)
        );
        let mut linear = SmoothChannel::default();
        linear.snap(3.1);
        linear.arm(&Arm::at(-3.1, 1.0));
        assert!(run(&mut linear, 0.5).iter().any(|v| *v > 0.0));
    }

    #[test]
    fn a_delay_holds_the_channel_before_the_tween() {
        let mut c = SmoothChannel::default();
        c.arm(&Arm {
            delay: 0.5,
            ..Arm::at(1.0, 1.0)
        });
        assert!(run(&mut c, 0.45).iter().all(|v| *v == 0.0));
        let frames = run(&mut c, 1.1);
        assert!((frames[frames.len() - 1] - 1.0).abs() < CHANNEL_EPS);
    }

    #[test]
    fn a_duration_bound_clamps_the_tween_not_the_gap() {
        let mut c = SmoothChannel::default();
        c.arm(&Arm {
            duration_bounds: Some((0.1, 0.5)),
            ..Arm::at(100.0, 7.5)
        });
        let frames = run(&mut c, 0.6);
        let arrived = frames
            .iter()
            .position(|v| (v - 100.0).abs() < CHANNEL_EPS)
            .expect("arrives");
        assert!((arrived as f32 / 60.0 - 0.5).abs() < 0.05);
    }
}
