use std::f32::consts::{FRAC_PI_2, PI, TAU};

use protocol::flags;
use server::Spawn;

use crate::ground::Ground;
use crate::region::{Rng, Scenario};

pub const RUN: f32 = 7.0;
pub const WALK: f32 = 2.5;
/// Height a walk may gain or lose per yard of ground, between samples this far apart.
const SLOPE: f32 = 1.2;
const SAMPLE_YD: f32 = 2.0;

/// What a bot does for one leg of its walk.
#[derive(Clone, Copy, Debug)]
pub enum Motion {
    Stand,
    /// Mouse-looking in place: the facing swings `swing` radians either way, `hz` times a second.
    Look {
        swing: f32,
        hz: f32,
    },
    /// Turning in place on the keyboard at `rate` radians a second, left when positive.
    Turn {
        rate: f32,
    },
    /// Straight ahead at `speed`, walking when `walk`, jumping at time `jump` when given.
    Run {
        speed: f32,
        walk: bool,
        jump: Option<u32>,
    },
    /// Running while mouse-turning at `rate` radians a second, left when positive.
    Arc {
        speed: f32,
        rate: f32,
    },
}

/// One leg of a walk, from `start` to `end` ms on the bots' shared clock.
#[derive(Clone, Copy, Debug)]
pub struct Leg {
    pub start: u32,
    pub end: u32,
    pub from: [f32; 2],
    pub facing: f32,
    pub motion: Motion,
}

impl Leg {
    /// Where the leg has taken the bot by `t`, and which way it faces.
    fn at(&self, t: u32) -> ([f32; 2], f32) {
        let dt = (t.clamp(self.start, self.end) - self.start) as f32 / 1000.0;
        let (f, [x, y]) = (self.facing, self.from);
        match self.motion {
            Motion::Stand => (self.from, f),
            Motion::Look { swing, hz } => (self.from, f + swing * (TAU * hz * dt).sin()),
            Motion::Turn { rate } => (self.from, f + rate * dt),
            Motion::Run { speed, .. } => {
                let d = speed * dt;
                ([x + d * f.cos(), y + d * f.sin()], f)
            }
            Motion::Arc { speed, rate } => {
                let (r, side) = (speed / rate.abs(), rate.signum() * FRAC_PI_2);
                let centre = [x + r * (f + side).cos(), y + r * (f + side).sin()];
                let a = f - side + rate * dt;
                (
                    [centre[0] + r * a.cos(), centre[1] + r * a.sin()],
                    f + rate * dt,
                )
            }
        }
    }

    fn flags(&self) -> u32 {
        match self.motion {
            Motion::Stand | Motion::Look { .. } => 0,
            Motion::Turn { rate } if rate > 0.0 => flags::TURN_LEFT,
            Motion::Turn { .. } => flags::TURN_RIGHT,
            Motion::Run { walk: true, .. } => flags::FORWARD | flags::WALK_MODE,
            Motion::Run { .. } | Motion::Arc { .. } => flags::FORWARD,
        }
    }
}

/// Where a bot is at an instant, and what its keys say.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub xy: [f32; 2],
    pub facing: f32,
    pub flags: u32,
    /// The current leg's jump, as its time and launch speed.
    pub jump: Option<(u32, f32)>,
    pub leg: usize,
}

/// When a lying bot lies: running three times too fast between `fast_from` and `fast_to`, and
/// once claiming a spot far from where it is at `teleport_at`.
#[derive(Clone, Copy, Debug)]
pub struct Lie {
    pub fast_from: u32,
    pub fast_to: u32,
    pub teleport_at: u32,
}

/// A bot's whole walk: where it is at any time, which is also where it honestly claims to be.
pub struct Track {
    legs: Vec<Leg>,
    pub lie: Option<Lie>,
}

impl Track {
    #[cfg(test)]
    pub fn of(legs: Vec<Leg>, lie: Option<Lie>) -> Self {
        Self { legs, lie }
    }

    fn leg(&self, t: u32) -> usize {
        self.legs
            .partition_point(|l| l.start <= t)
            .saturating_sub(1)
    }

    pub fn pose(&self, t: u32) -> Pose {
        let i = self.leg(t);
        let leg = &self.legs[i];
        let (xy, facing) = leg.at(t);
        let jump = match leg.motion {
            Motion::Run {
                jump: Some(at),
                speed,
                ..
            } => Some((at, speed)),
            _ => None,
        };
        Pose {
            xy,
            facing,
            flags: leg.flags(),
            jump,
            leg: i,
        }
    }

    pub fn xy(&self, t: u32) -> [f32; 2] {
        let leg = &self.legs[self.leg(t)];
        leg.at(t).0
    }

    /// Where the lie puts the bot at `t`, when it is lying then.
    pub fn lie_xy(&self, t: u32) -> Option<[f32; 2]> {
        let lie = self.lie?;
        (lie.fast_from..lie.fast_to)
            .contains(&t)
            .then(|| self.xy(lie.fast_from + 3 * (t - lie.fast_from)))
    }
}

/// Plans a walk from `spawn` starting at `start` ms and lasting past `until`: legs of running,
/// walking, arcs, standing, looking about and turning, over ground whose slope a player could
/// climb and inside the scenario's region. A liar walks long straight runs and gets a [`Lie`].
pub fn plan(
    s: &Scenario,
    ground: &Ground,
    spawn: &Spawn,
    span: (u32, u32),
    seed: u64,
    liar: bool,
) -> Track {
    let (start, until) = span;
    let mut p = Planner {
        s,
        ground,
        rng: Rng::new(seed),
    };
    let mut legs = vec![Leg {
        start,
        end: start + p.rng.range(300.0, 2000.0) as u32,
        from: [spawn.pos[0], spawn.pos[1]],
        facing: spawn.facing,
        motion: Motion::Stand,
    }];
    while legs.last().is_some_and(|l| l.end < until) {
        let last = legs[legs.len() - 1];
        let (from, facing) = last.at(last.end);
        let leg = if liar {
            p.liar_leg(last.end, from, facing)
        } else {
            p.next_leg(last.end, from, facing)
        };
        legs.push(leg);
    }
    let lie = liar.then(|| lie_on(&legs, start)).flatten();
    Track { legs, lie }
}

struct Planner<'a> {
    s: &'a Scenario,
    ground: &'a Ground,
    rng: Rng,
}

impl Planner<'_> {
    fn next_leg(&mut self, t: u32, from: [f32; 2], facing: f32) -> Leg {
        let roll = self.rng.range(0.0, 1.0);
        let planned = if roll < 0.55 {
            let walk = roll < 0.1;
            self.run_to(t, from, if walk { WALK } else { RUN }, walk, 3.0)
        } else if roll < 0.7 {
            self.arc(t, from, facing)
        } else {
            None
        };
        planned.unwrap_or_else(|| self.still(t, from, facing))
    }

    fn still(&mut self, t: u32, from: [f32; 2], facing: f32) -> Leg {
        let rng = &mut self.rng;
        let (motion, secs) = match rng.range(0.0, 3.0) as u32 {
            0 => (Motion::Stand, rng.range(1.0, 4.0)),
            1 => (
                Motion::Look {
                    swing: rng.range(0.3, 1.0),
                    hz: rng.range(0.3, 0.8),
                },
                rng.range(1.5, 4.0),
            ),
            _ => (
                Motion::Turn {
                    rate: if rng.chance(0.5) { PI } else { -PI },
                },
                rng.range(0.5, 1.5),
            ),
        };
        Leg {
            start: t,
            end: t + (secs * 1000.0) as u32,
            from,
            facing,
            motion,
        }
    }

    fn liar_leg(&mut self, t: u32, from: [f32; 2], facing: f32) -> Leg {
        let far = (self.s.reach * 0.6).min(40.0);
        self.run_to(t, from, RUN, false, far).unwrap_or(Leg {
            start: t,
            end: t + 1500,
            from,
            facing,
            motion: Motion::Stand,
        })
    }

    /// A straight run or walk to a climbable point of the region at least `min_yd` away.
    fn run_to(
        &mut self,
        t: u32,
        from: [f32; 2],
        speed: f32,
        walk: bool,
        min_yd: f32,
    ) -> Option<Leg> {
        let to = self
            .s
            .region
            .sample(self.ground, &mut self.rng, Some((from, self.s.reach)))?;
        let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
        let dist = dx.hypot(dy);
        if dist < min_yd {
            return None;
        }
        let ms = (dist / speed * 1000.0) as u32;
        let jump = (!walk && ms > 2500 && self.rng.chance(0.3))
            .then(|| t + self.rng.range(500.0, (ms - 1500) as f32) as u32);
        let leg = Leg {
            start: t,
            end: t + ms,
            from,
            facing: dy.atan2(dx).rem_euclid(TAU),
            motion: Motion::Run { speed, walk, jump },
        };
        self.climbable(&leg).then_some(leg)
    }

    fn arc(&mut self, t: u32, from: [f32; 2], facing: f32) -> Option<Leg> {
        let side = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
        let leg = Leg {
            start: t,
            end: t + self.rng.range(1500.0, 4000.0) as u32,
            from,
            facing,
            motion: Motion::Arc {
                speed: RUN,
                rate: self.rng.range(0.5, 1.2) * side,
            },
        };
        self.climbable(&leg).then_some(leg)
    }

    /// Whether every stretch of the leg stays in the region, on ground a player could climb or
    /// walk down without falling.
    fn climbable(&self, leg: &Leg) -> bool {
        let (Motion::Run { speed, .. } | Motion::Arc { speed, .. }) = leg.motion else {
            return true;
        };
        let step_ms = (SAMPLE_YD / speed * 1000.0).max(1.0) as usize;
        let mut last: Option<f32> = None;
        (leg.start..=leg.end)
            .step_by(step_ms)
            .chain([leg.end])
            .all(|t| {
                let (p, _) = leg.at(t);
                let Some(z) = self.ground.height(p[0], p[1]) else {
                    return false;
                };
                let ok = self.s.region.contains(self.ground, p)
                    && last.is_none_or(|l| (z - l).abs() <= SLOPE * SAMPLE_YD);
                last = Some(z);
                ok
            })
    }
}

/// The lies a liar tells: running too fast early in the first long run after eight seconds, and
/// a teleport a second into the first run after twenty.
fn lie_on(legs: &[Leg], start: u32) -> Option<Lie> {
    let long_run = |after: u32, ms: u32| {
        legs.iter().find(|l| {
            matches!(l.motion, Motion::Run { .. })
                && l.start >= start + after
                && l.end - l.start >= ms
        })
    };
    let fast = long_run(8000, 4000)?;
    let teleport = long_run(20_000, 2000)?;
    Some(Lie {
        fast_from: fast.start + 700,
        fast_to: fast.start + 2700,
        teleport_at: teleport.start + 1000,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg(start: u32, end: u32, motion: Motion) -> Leg {
        Leg {
            start,
            end,
            from: [0.0, 0.0],
            facing: 0.0,
            motion,
        }
    }

    #[test]
    fn a_run_goes_straight_at_its_speed_and_an_arc_keeps_its_radius() {
        let run = leg(
            0,
            2000,
            Motion::Run {
                speed: RUN,
                walk: false,
                jump: None,
            },
        );
        let (p, f) = run.at(1000);
        assert!((p[0] - 7.0).abs() < 1e-5 && p[1].abs() < 1e-5 && f == 0.0);
        let arc = leg(
            0,
            4000,
            Motion::Arc {
                speed: RUN,
                rate: 1.0,
            },
        );
        let centre = [0.0, 7.0];
        for t in [0, 500, 1571, 4000] {
            let (p, f) = arc.at(t);
            assert!((dist(p, centre) - 7.0).abs() < 1e-3, "t={t}");
            assert!((f - t as f32 / 1000.0).abs() < 1e-5);
        }
        let quarter = arc.at(1571).0;
        assert!((quarter[0] - 7.0).abs() < 0.01 && (quarter[1] - 7.0).abs() < 0.01);
    }

    #[test]
    fn a_lie_runs_three_times_ahead_only_inside_its_window() {
        let run = leg(
            0,
            20_000,
            Motion::Run {
                speed: RUN,
                walk: false,
                jump: None,
            },
        );
        let track = Track::of(
            vec![run],
            Some(Lie {
                fast_from: 1000,
                fast_to: 3000,
                teleport_at: 9000,
            }),
        );
        assert_eq!(track.lie_xy(999), None);
        let ahead = track.lie_xy(2000).expect("lying");
        assert!((ahead[0] - track.xy(4000)[0]).abs() < 1e-4);
        assert_eq!(track.lie_xy(3000), None);
    }

    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }
}
