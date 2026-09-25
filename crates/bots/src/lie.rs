use protocol::{Movement, flags};

/// Yards north of the map's centre a claim past every bound stands at.
const PAST_THE_BOUND_YD: f32 = 100_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Malformed {
    NotANumber,
    PastTheBound,
}

/// The clock a lying claim carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Clock {
    Body,
    /// Runs this many times as fast as the body's since the lie began.
    Rate(f32),
    /// Stands this many milliseconds behind the body's.
    Back(u32),
    /// Stands still at the lie's start; after the lie, the claims carry the body's again.
    Stall,
}

/// How a bot's claims part from what its body does, from `from_ms` until `to_ms` of its own
/// clock. Every field at its default tells the truth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lie {
    pub from_ms: u32,
    pub to_ms: u32,
    /// The claims run along the route this many times as far as the body since the lie began.
    pub factor: Option<f32>,
    /// Added to every claimed position.
    pub shift: [f32; 3],
    /// Only the lie's first claim is shifted.
    pub once: bool,
    /// Yards a second the claims climb since the lie began, with no jump.
    pub rise: f32,
    /// No claim is lower than the lie's first.
    pub hover: bool,
    /// Where the body stopped, the claims go on along its last run.
    pub through: bool,
    pub set_flags: u32,
    pub clear_flags: u32,
    pub clock: Clock,
    /// A jump claims this many times the speed it launched at.
    pub launch: f32,
    pub malformed: Option<Malformed>,
}

impl Default for Lie {
    fn default() -> Self {
        Self {
            from_ms: 0,
            to_ms: u32::MAX,
            factor: None,
            shift: [0.0; 3],
            once: false,
            rise: 0.0,
            hover: false,
            through: false,
            set_flags: 0,
            clear_flags: 0,
            clock: Clock::Body,
            launch: 1.0,
            malformed: None,
        }
    }
}

/// What a lie has told so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct Told {
    pub begun: bool,
    /// A claim carrying the shift has gone out.
    pub shifted: bool,
    pub first_z: f32,
}

/// Where a body's route takes it at a moment of its clock.
pub trait Route {
    /// The body's position at `t`, on the ground under it.
    fn at(&self, t: u32) -> [f32; 3];
    /// Where the body's last run would have taken it by `t` had it not stopped.
    fn through(&self, t: u32) -> [f32; 3];
}

impl Lie {
    pub fn holds_at(&self, t: u32) -> bool {
        (self.from_ms..self.to_ms).contains(&t)
    }

    /// How many times as far along its route as its body the lie's claims may get.
    pub fn reach(&self) -> f32 {
        let clock = match self.clock {
            Clock::Rate(r) => r,
            _ => 1.0,
        };
        self.factor.unwrap_or(1.0).max(clock).max(1.0)
    }

    /// The claim this lie makes of the body's `truth` at `t` of its clock, while it holds.
    pub fn tell(&self, told: &mut Told, t: u32, truth: &Movement, route: &impl Route) -> Movement {
        let mut m = *truth;
        let since = t - self.from_ms;
        let above_ground = truth.pos[2] - route.at(t)[2];
        let moved = match (self.through, self.factor) {
            (true, _) => Some(route.through(t)),
            (false, Some(f)) => Some(route.at(self.from_ms + (f * since as f32) as u32)),
            (false, None) => None,
        };
        if let Some(at) = moved {
            m.pos = [at[0], at[1], at[2] + above_ground];
        }
        if !self.once || !told.shifted {
            for (p, d) in m.pos.iter_mut().zip(self.shift) {
                *p += d;
            }
        }
        m.pos[2] += self.rise * since as f32 / 1000.0;
        if !told.begun {
            told.first_z = m.pos[2];
        }
        if self.hover {
            m.pos[2] = m.pos[2].max(told.first_z);
        }
        m.flags = (m.flags | self.set_flags) & !self.clear_flags;
        if m.flags & flags::SWIMMING == 0 {
            m.pitch = 0.0;
        }
        m.time = match self.clock {
            Clock::Body => t,
            Clock::Rate(r) => self.from_ms + (r * since as f32) as u32,
            Clock::Back(ms) => t.saturating_sub(ms),
            Clock::Stall => self.from_ms,
        };
        m.jump.xy_speed *= self.launch;
        match self.malformed {
            Some(Malformed::NotANumber) => m.pos[0] = f32::NAN,
            Some(Malformed::PastTheBound) => m.pos[0] = PAST_THE_BOUND_YD,
            None => {}
        }
        told.begun = true;
        m
    }
}

/// Whether two movements are the same to the bit, as a claim and its copy off the wire are.
pub fn same(a: &Movement, b: &Movement) -> bool {
    let bits = |m: &Movement| {
        let j = m.jump;
        [
            m.time,
            m.flags,
            m.pos[0].to_bits(),
            m.pos[1].to_bits(),
            m.pos[2].to_bits(),
            m.facing.to_bits(),
            m.pitch.to_bits(),
            m.fall_time,
            j.z_speed.to_bits(),
            j.cos.to_bits(),
            j.sin.to_bits(),
            j.xy_speed.to_bits(),
        ]
    };
    bits(a) == bits(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Downhill;

    impl Route for Downhill {
        fn at(&self, t: u32) -> [f32; 3] {
            [t as f32 / 100.0, 0.0, -(t as f32) / 1000.0]
        }

        fn through(&self, t: u32) -> [f32; 3] {
            [t as f32 / 50.0, 0.0, 0.0]
        }
    }

    fn truth(t: u32) -> Movement {
        Movement {
            time: t,
            flags: flags::FORWARD,
            pos: Downhill.at(t),
            ..Movement::default()
        }
    }

    fn tell(lie: &Lie, times: &[u32]) -> Vec<Movement> {
        let mut told = Told::default();
        times
            .iter()
            .map(|&t| lie.tell(&mut told, t, &truth(t), &Downhill))
            .collect()
    }

    fn from_one_second(lie: Lie) -> Lie {
        Lie {
            from_ms: 1000,
            ..lie
        }
    }

    fn at(p: [f32; 3], want: [f32; 3]) -> bool {
        p.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-4)
    }

    #[test]
    fn a_lie_left_at_its_defaults_tells_the_truth() {
        let times = [1000, 1500, 9000];
        let claims = tell(&from_one_second(Lie::default()), &times);
        for (t, m) in times.into_iter().zip(claims) {
            assert!(same(&m, &truth(t)), "{t}: {m:?}");
        }
    }

    #[test]
    fn a_speed_lie_runs_ahead_on_the_ground_and_a_shift_once_goes_out_once() {
        let fast = from_one_second(Lie {
            factor: Some(3.0),
            ..Lie::default()
        });
        assert!(at(tell(&fast, &[2000])[0].pos, Downhill.at(4000)));
        let jump = Lie {
            shift: [300.0, 0.0, 0.0],
            once: true,
            ..Lie::default()
        };
        let mut told = Told::default();
        let first = jump.tell(&mut told, 500, &truth(500), &Downhill);
        told.shifted = true;
        let second = jump.tell(&mut told, 550, &truth(550), &Downhill);
        assert!(at(first.pos, [305.0, 0.0, -0.5]));
        assert!(same(&second, &truth(550)));
    }

    #[test]
    fn a_hover_keeps_its_first_height_as_the_ground_falls_and_a_rise_climbs() {
        let hover = from_one_second(Lie {
            hover: true,
            ..Lie::default()
        });
        let claims = tell(&hover, &[1000, 3000, 9000]);
        assert!(
            claims.iter().all(|m| (m.pos[2] + 1.0).abs() < 1e-6),
            "{claims:?}"
        );
        let rise = from_one_second(Lie {
            rise: 2.0,
            ..Lie::default()
        });
        assert!(at(tell(&rise, &[3000])[0].pos, [30.0, 0.0, -3.0 + 4.0]));
        let through = from_one_second(Lie {
            through: true,
            ..Lie::default()
        });
        assert!(at(tell(&through, &[2000])[0].pos, [40.0, 0.0, 0.0]));
    }

    #[test]
    fn clocks_run_fast_stand_back_or_stall() {
        let at = |clock: Clock, t: u32| {
            let lie = from_one_second(Lie {
                clock,
                ..Lie::default()
            });
            tell(&lie, &[t])[0].time
        };
        assert_eq!(at(Clock::Rate(1.25), 5000), 6000);
        assert_eq!(at(Clock::Back(700), 5000), 4300);
        assert_eq!(at(Clock::Stall, 5000), 1000);
    }

    #[test]
    fn flags_are_set_and_cleared_and_a_malformed_claim_is_still_itself() {
        let odd = Lie {
            set_flags: flags::WALK_MODE | flags::FALLING,
            clear_flags: flags::FORWARD,
            malformed: Some(Malformed::NotANumber),
            ..Lie::default()
        };
        let m = tell(&odd, &[100])[0];
        assert_eq!(m.flags, flags::WALK_MODE | flags::FALLING);
        assert!(m.pos[0].is_nan() && same(&m, &m));
    }
}
