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
    /// The fraction of extra ground a claim may cover beyond its speed.
    pub tolerance: f32,
    /// Yards a claim may cover beyond its speed and tolerance, for a client's frame timing.
    pub slack: f32,
    /// Height gained per yard of ground on the steepest walkable slope.
    pub climb: f32,
    /// Height gained on top of the slope: a step up and a jump's apex, yards.
    pub rise: f32,
    /// The fastest fall, yards per second.
    pub fall: f32,
    /// How far a claim may lead the client's pinned clock without spending budget, ms.
    pub clock_slack_ms: u32,
    /// How far in all the pin may follow claims that lead it by more than the slack, ms.
    pub clock_budget_ms: u32,
    /// Coordinates past this many yards from the map's centre are malformed.
    pub bound: f32,
    /// When false, a well-formed claim that acknowledges the latest correction is accepted
    /// unchecked.
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
            clock_budget_ms: 10_000,
            bound: 17_066.666,
            check: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    Stale,
    Refuse(Why),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Malformed,
    /// Earlier than the last accepted movement, or further ahead of the pinned clock than the
    /// slack and the budget left allow.
    Clock,
    /// Further over the ground than the speed allows.
    Speed,
    Climb,
    Fall,
    /// A jump launched faster than a run.
    Launch,
}

impl Why {
    pub const ALL: [Self; 6] = [
        Self::Malformed,
        Self::Clock,
        Self::Speed,
        Self::Climb,
        Self::Fall,
        Self::Launch,
    ];
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockPin {
    pub client_ms: u32,
    pub server_ms: u32,
}

impl ClockPin {
    pub fn lead_ms(self, client_ms: u32, server_ms: u32) -> i64 {
        let client = i64::from(client_ms) - i64::from(self.client_ms);
        client - (i64::from(server_ms) - i64::from(self.server_ms))
    }
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

    pub fn judge(&self, body: &Body, claim: &Claim, received_ms: u32) -> Verdict {
        if claim.ack != body.correction_seq {
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
        let budget = self.clock_budget_ms.saturating_sub(body.clock_spent_ms);
        let lead_allowed = i64::from(self.clock_slack_ms) + i64::from(budget);
        let ahead = body
            .clock
            .is_some_and(|pin| pin.lead_ms(m.time, received_ms) > lead_allowed);
        if m.time < last.time || ahead {
            return Verdict::Refuse(Why::Clock);
        }
        let launched = m.flags & flags::FALLING != 0 && last.flags & flags::FALLING == 0;
        if launched && m.jump.xy_speed > self.run * (1.0 + self.tolerance) {
            return Verdict::Refuse(Why::Launch);
        }
        let dt = (m.time - last.time) as f32 / 1000.0;
        let dz = m.pos[2] - last.pos[2];
        let ground = ground_between(last, m);
        if ground > self.ground_allowed(last, m) {
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

    /// Pins `body`'s clock to an accepted claim, or moves the pin forward by as much as the
    /// claim led it past the slack, spending that from the budget.
    pub fn pin_clock(&self, body: &mut Body, client_ms: u32, received_ms: u32) {
        let pin = match body.clock {
            None => ClockPin {
                client_ms,
                server_ms: received_ms,
            },
            Some(pin) => {
                let lead = pin.lead_ms(client_ms, received_ms);
                let excess = (lead - i64::from(self.clock_slack_ms)).max(0) as u32;
                body.clock_spent_ms += excess;
                ClockPin {
                    client_ms: pin.client_ms + excess,
                    ..pin
                }
            }
        };
        body.clock = Some(pin);
    }

    /// How far over the ground a mover may go from `last` to `m`, yards. The flags may have
    /// changed anywhere between the two, so the faster of their speeds holds.
    pub fn ground_allowed(&self, last: &Movement, m: &Movement) -> f32 {
        let dt = m.time.saturating_sub(last.time) as f32 / 1000.0;
        let speed = self.speed(last.flags).max(self.speed(m.flags));
        speed * dt * (1.0 + self.tolerance) + self.slack
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

pub fn ground_between(a: &Movement, b: &Movement) -> f32 {
    (b.pos[0] - a.pos[0]).hypot(b.pos[1] - a.pos[1])
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
            clock: Some(ClockPin::default()),
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
        let still_walking = claim(1500, flags::FORWARD | flags::WALK_MODE, [3.5, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&walking, &still_walking, 1500),
            Verdict::Refuse(Why::Speed)
        );
        let standing = last_at([0.0, 0.0, 0.0], 1000, 0);
        let started_late = claim(1200, flags::FORWARD, [1.4, 0.0, 0.0]);
        assert_eq!(rules.judge(&standing, &started_late, 1200), Verdict::Accept);
    }

    #[test]
    fn a_teleport_and_a_rewind_are_refused() {
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
    }

    #[test]
    fn a_pin_made_late_catches_up_once_but_a_fast_clock_runs_out_of_budget() {
        let rules = Rules::default();
        let mut body = last_at([0.0, 0.0, 0.0], 1000, flags::FORWARD);
        body.clock = Some(ClockPin {
            client_ms: 1000,
            server_ms: 6000,
        });
        let prompt = claim(7000, flags::FORWARD, [40.0, 0.0, 0.0]);
        assert_eq!(rules.judge(&body, &prompt, 7000), Verdict::Accept);
        rules.pin_clock(&mut body, 7000, 7000);
        assert_eq!(body.clock.map(|p| p.lead_ms(8000, 8000)), Some(1000));

        let mut body = last_at([0.0, 0.0, 0.0], 0, flags::FORWARD);
        let refused_at = (1..200u32).find(|&k| {
            let (client, server) = (k * 600, k * 500);
            let c = claim(client, flags::FORWARD, [0.0, 0.0, 0.0]);
            match rules.judge(&body, &c, server) {
                Verdict::Accept => {
                    rules.pin_clock(&mut body, client, server);
                    body.movement.time = client;
                    false
                }
                _ => true,
            }
        });
        assert_eq!(
            refused_at,
            Some(111),
            "a clock 20 % fast banks 11 s, then is refused"
        );
    }

    #[test]
    fn claims_before_the_latest_correction_are_stale_and_unchecked_claims_pass() {
        let mut rules = Rules::default();
        let mut body = last_at([0.0, 0.0, 0.0], 1000, 0);
        body.correction_seq = 2;
        let teleport = claim(1500, 0, [500.0, 0.0, 0.0]);
        assert_eq!(rules.judge(&body, &teleport, 1500), Verdict::Stale);
        body.correction_seq = 0;
        rules.check = false;
        assert_eq!(rules.judge(&body, &teleport, 1500), Verdict::Accept);
        let nan = claim(1500, 0, [f32::NAN, 0.0, 0.0]);
        assert_eq!(
            rules.judge(&body, &nan, 1500),
            Verdict::Refuse(Why::Malformed)
        );
    }
}
