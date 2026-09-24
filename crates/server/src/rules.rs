use protocol::{Claim, Movement, flags};

use crate::world::Body;

/// The mover's speeds, yards per second, and how much slack the check of a claim allows.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub walk: f32,
    pub run: f32,
    pub run_back: f32,
    pub swim: f32,
    pub swim_back: f32,
    /// A claim may cover this fraction more ground than its speed allows...
    pub tolerance: f32,
    /// ...and this many yards more, for a client's frame timing.
    pub slack: f32,
    /// Height gained per yard of ground on the steepest walkable slope.
    pub climb: f32,
    /// Height gained on top of the slope: a step up and a jump's apex, yards.
    pub rise: f32,
    /// The fastest fall, yards per second.
    pub fall: f32,
    /// How far a client's clock may run ahead of the server's between two claims, ms.
    pub clock_slack_ms: u32,
    /// Coordinates past this many yards from the map's centre are malformed.
    pub bound: f32,
    /// When false every well-formed claim is accepted.
    pub check: bool,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            walk: 2.5,
            run: 7.0,
            run_back: 4.5,
            swim: 4.722_222,
            swim_back: 2.5,
            tolerance: 0.1,
            slack: 0.5,
            climb: 1.2,
            rise: 2.7,
            fall: 60.148,
            clock_slack_ms: 1000,
            bound: 17_066.666,
            check: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    /// Made before the client took its latest correction: ignored, not refused.
    Stale,
    Refuse(Why),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Malformed,
    /// Earlier than the last claim, or ahead of the server's clock.
    Clock,
    /// Further over the ground than the speed allows.
    Speed,
    Climb,
    Fall,
    /// A jump launched faster than a run.
    Launch,
}

impl Rules {
    /// How fast a mover whose flags are `f` crosses the ground; an arc keeps its launch speed,
    /// which is at most a run.
    pub fn speed(&self, f: u32) -> f32 {
        let backing = f & (flags::FORWARD | flags::BACKWARD) == flags::BACKWARD;
        if f & flags::FALLING != 0 {
            self.run
        } else if f & flags::ANY_MOVE == 0 {
            0.0
        } else if f & flags::SWIMMING != 0 {
            if backing { self.swim_back } else { self.swim }
        } else if f & flags::WALK_MODE != 0 {
            self.walk
        } else if backing {
            self.run_back
        } else {
            self.run
        }
    }

    /// Whether `claim`, received at server time `at_ms`, may move `body` on from its last
    /// accepted state.
    pub fn judge(&self, body: &Body, claim: &Claim, at_ms: u32) -> Verdict {
        if claim.ack != body.seq {
            return Verdict::Stale;
        }
        let m = &claim.movement;
        if !self.well_formed(m) {
            return Verdict::Refuse(Why::Malformed);
        }
        if !self.check {
            return Verdict::Accept;
        }
        let last = &body.movement;
        let ahead = m.time.saturating_sub(body.clock_time);
        let elapsed = at_ms.saturating_sub(body.clock_at);
        if m.time < last.time || (body.clocked && ahead > elapsed + self.clock_slack_ms) {
            return Verdict::Refuse(Why::Clock);
        }
        let launched = m.flags & flags::FALLING != 0 && last.flags & flags::FALLING == 0;
        if launched && m.jump.xy_speed > self.run * (1.0 + self.tolerance) {
            return Verdict::Refuse(Why::Launch);
        }
        let dt = (m.time - last.time) as f32 / 1000.0;
        let [dx, dy, dz] = [0, 1, 2].map(|i| m.pos[i] - last.pos[i]);
        let ground = dx.hypot(dy);
        if ground > self.speed(last.flags) * dt * (1.0 + self.tolerance) + self.slack {
            return Verdict::Refuse(Why::Speed);
        }
        if dz > ground * self.climb + self.rise {
            return Verdict::Refuse(Why::Climb);
        }
        if -dz > self.fall * dt + ground * self.climb + self.rise {
            return Verdict::Refuse(Why::Fall);
        }
        Verdict::Accept
    }

    fn well_formed(&self, m: &Movement) -> bool {
        let j = m.jump;
        let finite = [m.pos[0], m.pos[1], m.pos[2], m.facing, m.pitch]
            .into_iter()
            .chain([j.z_speed, j.cos, j.sin, j.xy_speed])
            .all(f32::is_finite);
        finite
            && m.pos.iter().all(|c| c.abs() <= self.bound)
            && m.facing.abs() <= 4.0 * std::f32::consts::PI
            && m.pitch.abs() <= std::f32::consts::PI
            && j.xy_speed >= 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last_at(pos: [f32; 3], time: u32, flags: u32) -> Body {
        Body {
            movement: Movement {
                time,
                flags,
                pos,
                ..Movement::default()
            },
            clocked: true,
            clock_time: 0,
            clock_at: 0,
            ..Body::default()
        }
    }

    fn claim(time: u32, flags: u32, pos: [f32; 3]) -> Claim {
        Claim {
            ack: 0,
            movement: Movement {
                time,
                flags,
                pos,
                ..Movement::default()
            },
        }
    }

    #[test]
    fn a_run_within_its_speed_passes_and_a_faster_one_is_refused() {
        let rules = Rules::default();
        let body = last_at([0.0, 0.0, 0.0], 1000, flags::FORWARD);
        let honest = claim(1500, flags::FORWARD, [3.5, 0.0, 0.2]);
        assert_eq!(rules.judge(&body, &honest, 1500), Verdict::Accept);
        let fast = claim(1500, flags::FORWARD, [7.0, 0.0, 0.0]);
        assert_eq!(rules.judge(&body, &fast, 1500), Verdict::Refuse(Why::Speed));
        let walking = last_at([0.0, 0.0, 0.0], 1000, flags::FORWARD | flags::WALK_MODE);
        assert_eq!(
            rules.judge(&walking, &honest, 1500),
            Verdict::Refuse(Why::Speed)
        );
    }

    #[test]
    fn a_teleport_a_rewind_and_a_fast_clock_are_refused() {
        let rules = Rules::default();
        let body = last_at([0.0, 0.0, 0.0], 1000, 0);
        let teleport = claim(1500, 0, [200.0, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&body, &teleport, 1500),
            Verdict::Refuse(Why::Speed)
        );
        let rewind = claim(900, 0, [0.0, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&body, &rewind, 1500),
            Verdict::Refuse(Why::Clock)
        );
        let running = last_at([0.0, 0.0, 0.0], 1000, flags::FORWARD);
        let ahead = claim(5000, flags::FORWARD, [28.0, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&running, &ahead, 1500),
            Verdict::Refuse(Why::Clock)
        );
        assert_eq!(rules.judge(&running, &ahead, 4000), Verdict::Accept);
    }

    #[test]
    fn slopes_pass_and_climbs_and_sinks_do_not() {
        let rules = Rules::default();
        let body = last_at([0.0, 0.0, 10.0], 1000, flags::FORWARD);
        let uphill = claim(1500, flags::FORWARD, [3.5, 0.0, 13.0]);
        assert_eq!(rules.judge(&body, &uphill, 1500), Verdict::Accept);
        let up = claim(1500, flags::FORWARD, [0.0, 0.0, 20.0]);
        assert_eq!(rules.judge(&body, &up, 1500), Verdict::Refuse(Why::Climb));
        let fall = claim(2000, flags::FALLING, [0.0, 0.0, -30.0]);
        assert_eq!(rules.judge(&body, &fall, 2000), Verdict::Accept);
        let sink = claim(1100, flags::FORWARD, [0.0, 0.0, -20.0]);
        assert_eq!(rules.judge(&body, &sink, 1100), Verdict::Refuse(Why::Fall));
    }

    #[test]
    fn claims_before_the_latest_correction_are_stale_and_unchecked_claims_pass() {
        let mut rules = Rules::default();
        let mut body = last_at([0.0, 0.0, 0.0], 1000, 0);
        body.seq = 2;
        let teleport = claim(1500, 0, [500.0, 0.0, 0.0]);
        assert_eq!(rules.judge(&body, &teleport, 1500), Verdict::Stale);
        body.seq = 0;
        rules.check = false;
        assert_eq!(rules.judge(&body, &teleport, 1500), Verdict::Accept);
        let nan = claim(1500, 0, [f32::NAN, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&body, &nan, 1500),
            Verdict::Refuse(Why::Malformed)
        );
    }
}
