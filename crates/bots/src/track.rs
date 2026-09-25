use std::f32::consts::{FRAC_PI_2, PI, TAU};

use libm::{cosf, sinf};
use protocol::flags;
use server::Spawn;

use crate::ground::Ground;
use crate::lie::Lie;
use crate::region::{Place, Square, XorShift64Star};

pub const RUN: f32 = 7.0;
pub const WALK: f32 = 2.5;
const SAMPLE_YD: f32 = 2.0;
const FAST_LIE_FACTOR: f32 = 3.0;
const TELEPORT_YD: f32 = 200.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gait {
    Run,
    Walk,
    Swim,
}

#[derive(Clone, Copy, Debug)]
pub enum Motion {
    Stand,
    MouseLook {
        amplitude: f32,
        hz: f32,
    },
    KeyTurn {
        left_rad_per_s: f32,
    },
    Run {
        speed: f32,
        gait: Gait,
        jump_at: Option<u32>,
    },
    Arc {
        speed: f32,
        left_rad_per_s: f32,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Leg {
    pub start_ms: u32,
    pub end_ms: u32,
    pub from: [f32; 2],
    pub facing: f32,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug)]
pub struct Spot {
    pub xy: [f32; 2],
    pub facing: f32,
}

impl Leg {
    fn at(&self, t: u32) -> Spot {
        self.unclamped(t.clamp(self.start_ms, self.end_ms))
    }

    fn unclamped(&self, t: u32) -> Spot {
        let dt = t.saturating_sub(self.start_ms) as f32 / 1000.0;
        let (f, [x, y]) = (self.facing, self.from);
        let (xy, facing) = match self.motion {
            Motion::Stand => (self.from, f),
            Motion::MouseLook { amplitude, hz } => (self.from, f + amplitude * sinf(TAU * hz * dt)),
            Motion::KeyTurn { left_rad_per_s } => (self.from, f + left_rad_per_s * dt),
            Motion::Run { speed, .. } => {
                let d = speed * dt;
                ([x + d * cosf(f), y + d * sinf(f)], f)
            }
            Motion::Arc {
                speed,
                left_rad_per_s: rate,
            } => {
                let (r, side) = (speed / rate.abs(), rate.signum() * FRAC_PI_2);
                let centre = [x + r * cosf(f + side), y + r * sinf(f + side)];
                let a = f - side + rate * dt;
                (
                    [centre[0] + r * cosf(a), centre[1] + r * sinf(a)],
                    f + rate * dt,
                )
            }
        };
        Spot { xy, facing }
    }

    fn flags(&self) -> u32 {
        match self.motion {
            Motion::Stand | Motion::MouseLook { .. } => 0,
            Motion::KeyTurn { left_rad_per_s } if left_rad_per_s > 0.0 => flags::TURN_LEFT,
            Motion::KeyTurn { .. } => flags::TURN_RIGHT,
            Motion::Run { gait, .. } => match gait {
                Gait::Run => flags::FORWARD,
                Gait::Walk => flags::FORWARD | flags::WALK_MODE,
                Gait::Swim => flags::FORWARD | flags::SWIMMING,
            },
            Motion::Arc { .. } => flags::FORWARD,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Launch {
    pub at_ms: u32,
    pub speed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub spot: Spot,
    pub flags: u32,
    pub jump: Option<Launch>,
    pub leg: usize,
}

/// Where a bot goes, leg by leg, on its own clock; with `surface`, over the top of any water.
#[derive(Clone, Debug)]
pub struct Track {
    legs: Vec<Leg>,
    pub surface: bool,
}

impl Track {
    pub fn of(legs: Vec<Leg>) -> Self {
        Self {
            legs,
            surface: false,
        }
    }

    fn leg(&self, t: u32) -> usize {
        self.legs
            .partition_point(|l| l.start_ms <= t)
            .saturating_sub(1)
    }

    pub fn pose(&self, t: u32) -> Pose {
        let i = self.leg(t);
        let leg = &self.legs[i];
        let jump = match leg.motion {
            Motion::Run {
                jump_at: Some(at_ms),
                speed,
                ..
            } => Some(Launch { at_ms, speed }),
            _ => None,
        };
        Pose {
            spot: leg.at(t),
            flags: leg.flags(),
            jump,
            leg: i,
        }
    }

    pub fn xy(&self, t: u32) -> [f32; 2] {
        self.legs[self.leg(t)].at(t).xy
    }

    /// Where the last run begun by `t` would have taken the body by `t` had it gone on.
    pub fn through_xy(&self, t: u32) -> [f32; 2] {
        let begun_run = |l: &&Leg| matches!(l.motion, Motion::Run { .. }) && l.start_ms <= t;
        match self.legs.iter().rev().find(begun_run) {
            Some(run) => run.unclamped(t).xy,
            None => self.xy(t),
        }
    }

    pub fn footing_z(&self, ground: &Ground, x: f32, y: f32) -> Option<f32> {
        if self.surface {
            ground.surface(x, y)
        } else {
            ground.height(x, y)
        }
    }

    /// A straight run from `from` along its facing, from `start_ms` until `until_ms` or until it
    /// has covered its `stop_yd`, then standing.
    pub fn line(from: &Spawn, pace: &Pace, start_ms: u32, until_ms: u32) -> Self {
        let stop_ms = pace
            .stop_yd
            .map_or(until_ms, |yd| start_ms + (yd / pace.speed * 1000.0) as u32)
            .min(until_ms);
        let stride = pace.jump_every_ms.unwrap_or(u32::MAX);
        let mut legs = Vec::new();
        let mut at = [from.pos[0], from.pos[1]];
        let mut t = start_ms;
        while t < stop_ms {
            let end = t.saturating_add(stride).min(stop_ms);
            let jump_at = pace
                .jump_every_ms
                .map(|_| t + JUMP_AFTER_MS)
                .filter(|&j| j + JUMP_LANDS_WITHIN_MS <= end);
            let run = Leg {
                start_ms: t,
                end_ms: end,
                from: at,
                facing: from.facing,
                motion: Motion::Run {
                    speed: pace.speed,
                    gait: pace.gait,
                    jump_at,
                },
            };
            at = run.at(end).xy;
            legs.push(run);
            t = end;
        }
        legs.push(Leg {
            start_ms: t,
            end_ms: until_ms.max(t),
            from: at,
            facing: from.facing,
            motion: Motion::Stand,
        });
        Self {
            legs,
            surface: pace.surface,
        }
    }
}

const JUMP_AFTER_MS: u32 = 500;
const JUMP_LANDS_WITHIN_MS: u32 = 1000;

#[derive(Clone, Copy, Debug)]
pub struct Pace {
    pub speed: f32,
    pub gait: Gait,
    pub stop_yd: Option<f32>,
    pub jump_every_ms: Option<u32>,
    pub surface: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Walk {
    pub start_ms: u32,
    pub until_ms: u32,
    pub seed: u64,
    pub run_speed: f32,
    pub long_runs: bool,
}

pub fn plan(s: &Place, ground: &Ground, spawn: &Spawn, walk: &Walk) -> Track {
    let mut p = Planner {
        s,
        ground,
        rng: XorShift64Star::new(walk.seed),
        slope: server::Rules::default().climb,
        run: walk.run_speed,
    };
    let mut legs = vec![Leg {
        start_ms: walk.start_ms,
        end_ms: walk.start_ms + p.rng.range(300.0, 2000.0) as u32,
        from: [spawn.pos[0], spawn.pos[1]],
        facing: spawn.facing,
        motion: Motion::Stand,
    }];
    while legs.last().is_some_and(|l| l.end_ms < walk.until_ms) {
        let last = legs[legs.len() - 1];
        let end = last.at(last.end_ms);
        let leg = if walk.long_runs {
            p.liar_leg(last.end_ms, end)
        } else {
            p.next_leg(last.end_ms, end)
        };
        legs.push(leg);
    }
    Track::of(legs)
}

struct Planner<'a> {
    s: &'a Place,
    ground: &'a Ground,
    rng: XorShift64Star,
    slope: f32,
    run: f32,
}

impl Planner<'_> {
    fn next_leg(&mut self, t: u32, from: Spot) -> Leg {
        let roll = self.rng.range(0.0, 1.0);
        let planned = if roll < 0.55 {
            let (speed, gait) = if roll < 0.1 {
                (WALK, Gait::Walk)
            } else {
                (self.run, Gait::Run)
            };
            self.run_to(t, from.xy, speed, gait, 3.0)
        } else if roll < 0.7 {
            self.arc(t, from)
        } else {
            None
        };
        planned.unwrap_or_else(|| self.still(t, from))
    }

    fn still(&mut self, t: u32, from: Spot) -> Leg {
        let rng = &mut self.rng;
        let (motion, secs) = match rng.range(0.0, 3.0) as u32 {
            0 => (Motion::Stand, rng.range(1.0, 4.0)),
            1 => (
                Motion::MouseLook {
                    amplitude: rng.range(0.3, 1.0),
                    hz: rng.range(0.3, 0.8),
                },
                rng.range(1.5, 4.0),
            ),
            _ => (
                Motion::KeyTurn {
                    left_rad_per_s: if rng.chance(0.5) { PI } else { -PI },
                },
                rng.range(0.5, 1.5),
            ),
        };
        Leg {
            start_ms: t,
            end_ms: t + (secs * 1000.0) as u32,
            from: from.xy,
            facing: from.facing,
            motion,
        }
    }

    fn liar_leg(&mut self, t: u32, from: Spot) -> Leg {
        let far = (self.s.leg_reach * 0.6).min(40.0);
        self.run_to(t, from.xy, self.run, Gait::Run, far)
            .unwrap_or(Leg {
                start_ms: t,
                end_ms: t + 1500,
                from: from.xy,
                facing: from.facing,
                motion: Motion::Stand,
            })
    }

    fn run_to(
        &mut self,
        t: u32,
        from: [f32; 2],
        speed: f32,
        gait: Gait,
        min_yd: f32,
    ) -> Option<Leg> {
        let around = Square {
            centre: from,
            half_side: self.s.leg_reach,
        };
        let to = self
            .s
            .region
            .sample(self.ground, &mut self.rng, Some(around))?;
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let dist = dx.hypot(dy);
        if dist < min_yd {
            return None;
        }
        let ms = (dist / speed * 1000.0) as u32;
        let jump_at = (gait == Gait::Run && ms > 2500 && self.rng.chance(0.3))
            .then(|| t + self.rng.range(500.0, (ms - 1500) as f32) as u32);
        let leg = Leg {
            start_ms: t,
            end_ms: t + ms,
            from,
            facing: dy.atan2(dx).rem_euclid(TAU),
            motion: Motion::Run {
                speed,
                gait,
                jump_at,
            },
        };
        self.walkable(&leg).then_some(leg)
    }

    fn arc(&mut self, t: u32, from: Spot) -> Option<Leg> {
        let side = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
        let leg = Leg {
            start_ms: t,
            end_ms: t + self.rng.range(1500.0, 4000.0) as u32,
            from: from.xy,
            facing: from.facing,
            motion: Motion::Arc {
                speed: self.run,
                left_rad_per_s: self.rng.range(0.5, 1.2) * side,
            },
        };
        self.walkable(&leg).then_some(leg)
    }

    fn walkable(&self, leg: &Leg) -> bool {
        let (Motion::Run { speed, .. } | Motion::Arc { speed, .. }) = leg.motion else {
            return true;
        };
        let step_ms = (SAMPLE_YD / speed * 1000.0).max(1.0) as usize;
        let mut last: Option<f32> = None;
        (leg.start_ms..=leg.end_ms)
            .step_by(step_ms)
            .chain([leg.end_ms])
            .all(|t| {
                let p = leg.at(t).xy;
                let Some(z) = self.ground.height(p[0], p[1]) else {
                    return false;
                };
                let ok = self.s.region.contains(self.ground, p)
                    && last.is_none_or(|l| (z - l).abs() <= self.slope * SAMPLE_YD);
                last = Some(z);
                ok
            })
    }
}

/// None if the track has no runs long enough to lie on.
pub fn crowd_liar_lies(track: &Track, start_ms: u32) -> Vec<Lie> {
    let long_run = |after: u32, ms: u32| {
        track.legs.iter().find(|l| {
            matches!(l.motion, Motion::Run { .. })
                && l.start_ms >= start_ms + after
                && l.end_ms - l.start_ms >= ms
        })
    };
    let (Some(fast), Some(teleport)) = (long_run(8000, 4000), long_run(20_000, 2000)) else {
        return Vec::new();
    };
    vec![
        Lie {
            from_ms: fast.start_ms + 700,
            to_ms: fast.start_ms + 2700,
            factor: Some(FAST_LIE_FACTOR),
            ..Lie::default()
        },
        Lie {
            from_ms: teleport.start_ms + 1000,
            shift: [TELEPORT_YD, 0.0, 0.0],
            shift_once: true,
            ..Lie::default()
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(start_ms: u32, end_ms: u32, motion: Motion) -> Leg {
        Leg {
            start_ms,
            end_ms,
            from: [0.0, 0.0],
            facing: 0.0,
            motion,
        }
    }

    fn run() -> Motion {
        Motion::Run {
            speed: RUN,
            gait: Gait::Run,
            jump_at: None,
        }
    }

    #[test]
    fn a_run_goes_straight_at_its_speed_and_an_arc_keeps_its_radius() {
        let p = leg(0, 2000, run()).at(1000);
        assert!((p.xy[0] - 7.0).abs() < 1e-5 && p.xy[1].abs() < 1e-5 && p.facing == 0.0);
        let arc = Motion::Arc {
            speed: RUN,
            left_rad_per_s: 1.0,
        };
        let arc = leg(0, 4000, arc);
        let centre = [0.0, 7.0];
        for t in [0, 500, 1571, 4000] {
            let p = arc.at(t);
            assert!((dist(p.xy, centre) - 7.0).abs() < 1e-3, "t={t}");
            assert!((p.facing - t as f32 / 1000.0).abs() < 1e-5);
        }
        let quarter = arc.at(1571).xy;
        assert!((quarter[0] - 7.0).abs() < 0.01 && (quarter[1] - 7.0).abs() < 0.01);
    }

    #[test]
    fn a_line_stops_where_it_is_told_and_its_run_goes_on_through_the_stop() {
        let from = Spawn {
            pos: [10.0, 0.0, 0.0],
            facing: 0.0,
        };
        let line = Pace {
            speed: RUN,
            gait: Gait::Walk,
            stop_yd: Some(14.0),
            jump_every_ms: Some(1500),
            surface: false,
        };
        let track = Track::line(&from, &line, 1000, 20_000);
        assert!((track.xy(2000)[0] - 17.0).abs() < 1e-4);
        assert!((track.xy(9000)[0] - 24.0).abs() < 1e-4, "stood at the stop");
        assert!((track.through_xy(9000)[0] - 66.0).abs() < 1e-3);
        assert_eq!(track.pose(1500).flags, flags::FORWARD | flags::WALK_MODE);
        assert_eq!(track.pose(9000).flags, 0);
        let jumps: Vec<u32> = track
            .legs
            .iter()
            .filter_map(|l| match l.motion {
                Motion::Run { jump_at, .. } => jump_at,
                _ => None,
            })
            .collect();
        assert_eq!(
            jumps,
            [1500],
            "the second stretch ends too soon to land a jump"
        );
    }

    #[test]
    fn a_crowd_liar_lies_on_its_long_runs() {
        let legs = vec![
            leg(0, 9000, Motion::Stand),
            leg(9000, 14_000, run()),
            leg(14_000, 21_000, Motion::Stand),
            leg(21_000, 24_000, run()),
        ];
        let lies = crowd_liar_lies(&Track::of(legs), 0);
        let windows: Vec<_> = lies
            .iter()
            .map(|l| (l.from_ms, l.to_ms, l.factor.map(|f| f as u32), l.shift_once))
            .collect();
        assert_eq!(
            windows,
            [
                (9700, 11_700, Some(3), false),
                (22_000, u32::MAX, None, true)
            ]
        );
    }

    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }
}
