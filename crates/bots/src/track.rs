use std::f32::consts::{FRAC_PI_2, PI, TAU};

use protocol::flags;
use server::Spawn;

use crate::ground::Ground;
use crate::region::{Scenario, Square, XorShift64Star};

pub const RUN: f32 = 7.0;
pub const WALK: f32 = 2.5;
const SAMPLE_YD: f32 = 2.0;
const FAST_LIE_FACTOR: u32 = 3;

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
        walk: bool,
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
pub struct Place {
    pub xy: [f32; 2],
    pub facing: f32,
}

impl Leg {
    fn at(&self, t: u32) -> Place {
        let dt = (t.clamp(self.start_ms, self.end_ms) - self.start_ms) as f32 / 1000.0;
        let (f, [x, y]) = (self.facing, self.from);
        let (xy, facing) = match self.motion {
            Motion::Stand => (self.from, f),
            Motion::MouseLook { amplitude, hz } => {
                (self.from, f + amplitude * (TAU * hz * dt).sin())
            }
            Motion::KeyTurn { left_rad_per_s } => (self.from, f + left_rad_per_s * dt),
            Motion::Run { speed, .. } => {
                let d = speed * dt;
                ([x + d * f.cos(), y + d * f.sin()], f)
            }
            Motion::Arc {
                speed,
                left_rad_per_s: rate,
            } => {
                let (r, side) = (speed / rate.abs(), rate.signum() * FRAC_PI_2);
                let centre = [x + r * (f + side).cos(), y + r * (f + side).sin()];
                let a = f - side + rate * dt;
                (
                    [centre[0] + r * a.cos(), centre[1] + r * a.sin()],
                    f + rate * dt,
                )
            }
        };
        Place { xy, facing }
    }

    fn flags(&self) -> u32 {
        match self.motion {
            Motion::Stand | Motion::MouseLook { .. } => 0,
            Motion::KeyTurn { left_rad_per_s } if left_rad_per_s > 0.0 => flags::TURN_LEFT,
            Motion::KeyTurn { .. } => flags::TURN_RIGHT,
            Motion::Run { walk: true, .. } => flags::FORWARD | flags::WALK_MODE,
            Motion::Run { .. } | Motion::Arc { .. } => flags::FORWARD,
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
    pub place: Place,
    pub flags: u32,
    pub jump: Option<Launch>,
    pub leg: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct Lie {
    pub fast_from: u32,
    pub fast_to: u32,
    pub teleport_at: u32,
}

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
            place: leg.at(t),
            flags: leg.flags(),
            jump,
            leg: i,
        }
    }

    pub fn xy(&self, t: u32) -> [f32; 2] {
        self.legs[self.leg(t)].at(t).xy
    }

    pub fn fast_lie_xy(&self, t: u32) -> Option<[f32; 2]> {
        let lie = self.lie?;
        (lie.fast_from..lie.fast_to)
            .contains(&t)
            .then(|| self.xy(lie.fast_from + FAST_LIE_FACTOR * (t - lie.fast_from)))
    }
}

pub fn plan(
    s: &Scenario,
    ground: &Ground,
    spawn: &Spawn,
    start_ms: u32,
    until_ms: u32,
    seed: u64,
    liar: bool,
) -> Track {
    let mut p = Planner {
        s,
        ground,
        rng: XorShift64Star::new(seed),
        slope: server::Rules::default().climb,
    };
    let mut legs = vec![Leg {
        start_ms,
        end_ms: start_ms + p.rng.range(300.0, 2000.0) as u32,
        from: [spawn.pos[0], spawn.pos[1]],
        facing: spawn.facing,
        motion: Motion::Stand,
    }];
    while legs.last().is_some_and(|l| l.end_ms < until_ms) {
        let last = legs[legs.len() - 1];
        let end = last.at(last.end_ms);
        let leg = if liar {
            p.liar_leg(last.end_ms, end)
        } else {
            p.next_leg(last.end_ms, end)
        };
        legs.push(leg);
    }
    let lie = liar.then(|| lie_on(&legs, start_ms)).flatten();
    Track { legs, lie }
}

struct Planner<'a> {
    s: &'a Scenario,
    ground: &'a Ground,
    rng: XorShift64Star,
    slope: f32,
}

impl Planner<'_> {
    fn next_leg(&mut self, t: u32, from: Place) -> Leg {
        let roll = self.rng.range(0.0, 1.0);
        let planned = if roll < 0.55 {
            let walk = roll < 0.1;
            self.run_to(t, from.xy, if walk { WALK } else { RUN }, walk, 3.0)
        } else if roll < 0.7 {
            self.arc(t, from)
        } else {
            None
        };
        planned.unwrap_or_else(|| self.still(t, from))
    }

    fn still(&mut self, t: u32, from: Place) -> Leg {
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

    fn liar_leg(&mut self, t: u32, from: Place) -> Leg {
        let far = (self.s.leg_reach * 0.6).min(40.0);
        self.run_to(t, from.xy, RUN, false, far).unwrap_or(Leg {
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
        walk: bool,
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
        let jump_at = (!walk && ms > 2500 && self.rng.chance(0.3))
            .then(|| t + self.rng.range(500.0, (ms - 1500) as f32) as u32);
        let leg = Leg {
            start_ms: t,
            end_ms: t + ms,
            from,
            facing: dy.atan2(dx).rem_euclid(TAU),
            motion: Motion::Run {
                speed,
                walk,
                jump_at,
            },
        };
        self.walkable(&leg).then_some(leg)
    }

    fn arc(&mut self, t: u32, from: Place) -> Option<Leg> {
        let side = if self.rng.chance(0.5) { 1.0 } else { -1.0 };
        let leg = Leg {
            start_ms: t,
            end_ms: t + self.rng.range(1500.0, 4000.0) as u32,
            from: from.xy,
            facing: from.facing,
            motion: Motion::Arc {
                speed: RUN,
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

fn lie_on(legs: &[Leg], start_ms: u32) -> Option<Lie> {
    let long_run = |after: u32, ms: u32| {
        legs.iter().find(|l| {
            matches!(l.motion, Motion::Run { .. })
                && l.start_ms >= start_ms + after
                && l.end_ms - l.start_ms >= ms
        })
    };
    let fast = long_run(8000, 4000)?;
    let teleport = long_run(20_000, 2000)?;
    Some(Lie {
        fast_from: fast.start_ms + 700,
        fast_to: fast.start_ms + 2700,
        teleport_at: teleport.start_ms + 1000,
    })
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
            walk: false,
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
    fn a_lie_runs_three_times_ahead_only_inside_its_window() {
        let lie = Lie {
            fast_from: 1000,
            fast_to: 3000,
            teleport_at: 9000,
        };
        let track = Track::of(vec![leg(0, 20_000, run())], Some(lie));
        assert_eq!(track.fast_lie_xy(999), None);
        let ahead = track.fast_lie_xy(2000).expect("lying");
        assert!((ahead[0] - track.xy(4000)[0]).abs() < 1e-4);
        assert_eq!(track.fast_lie_xy(3000), None);
    }

    fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
        (a[0] - b[0]).hypot(a[1] - b[1])
    }
}
