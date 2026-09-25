use std::collections::HashMap;
use std::f32::consts::TAU;

use libm::{atan2f, sqrtf};
use protocol::{Record, flags};

const CLOSE_YD: f32 = 2.0;
const SWING_WITHIN_YD: f32 = 4.0;
const SWING_EVERY_MS: u32 = 500;
const AIM_EVERY_MS: u32 = 250;

#[derive(Clone, Copy, Debug)]
struct Seen {
    id: u32,
    pos: [f32; 3],
    rooted: bool,
}

/// The entities in a bot's view as its batches have told it by the time they reach it.
#[derive(Default)]
pub struct Sight {
    by_slot: HashMap<u16, Seen>,
}

impl Sight {
    /// Takes in one record, its positions read against `here`, where the bot stands.
    pub fn see(&mut self, record: &Record<'_>, here: [f32; 3]) {
        match *record {
            Record::Appear {
                slot, id, state, ..
            } => {
                let seen = Seen {
                    id,
                    pos: state.pos.around(here).yards(),
                    rooted: state.flags & flags::ROOT != 0,
                };
                self.by_slot.insert(slot, seen);
            }
            Record::Move { slot, pos, .. } => {
                if let Some(seen) = self.by_slot.get_mut(&slot) {
                    seen.pos = pos.around(here).yards();
                }
            }
            Record::State { slot, state } => {
                if let Some(seen) = self.by_slot.get_mut(&slot) {
                    seen.pos = state.pos.around(here).yards();
                    seen.rooted = state.flags & flags::ROOT != 0;
                }
            }
            Record::Vanish { slot } => {
                self.by_slot.remove(&slot);
            }
            _ => {}
        }
    }

    /// The nearest to `at` of the entities not rooted, and how far it stands; the lowest id of a
    /// tie.
    fn nearest_free(&self, at: [f32; 2]) -> Option<(f32, [f32; 3])> {
        self.by_slot
            .values()
            .filter(|s| !s.rooted)
            .map(|s| {
                let (dx, dy) = (s.pos[0] - at[0], s.pos[1] - at[1]);
                (sqrtf(dx * dx + dy * dy), s.id, s.pos)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(d, _, pos)| (d, pos))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    /// Face this way, run this far along it, then stand.
    Toward { facing: f32, run_yd: f32 },
    /// Stand: there is no one to fight.
    Still,
}

/// When a fighter next picks its way, and when it next swings.
#[derive(Default)]
pub struct Fighter {
    aim_at: u32,
    swing_at: u32,
}

impl Fighter {
    /// At `t`, standing at `me`: a new aim when one is due, and whether to swing.
    pub fn frame(&mut self, t: u32, me: [f32; 2], sight: &Sight) -> (Option<Aim>, bool) {
        let target = sight.nearest_free(me);
        let aim = (t >= self.aim_at).then(|| {
            self.aim_at = t + AIM_EVERY_MS;
            target.map_or(Aim::Still, |(d, at)| Aim::Toward {
                facing: atan2f(at[1] - me[1], at[0] - me[0]).rem_euclid(TAU),
                run_yd: (d - CLOSE_YD).max(0.0),
            })
        });
        let swing = target.is_some_and(|(d, _)| d <= SWING_WITHIN_YD) && t >= self.swing_at;
        if swing {
            self.swing_at = t + SWING_EVERY_MS;
        }
        (aim, swing)
    }
}

#[cfg(test)]
mod tests {
    use protocol::{Appearance, Movement, State};

    use super::*;

    fn appear(slot: u16, id: u32, x: f32, y: f32, flags: u32) -> Record<'static> {
        let state = State::of(&Movement {
            flags,
            pos: [x, y, 0.0],
            ..Movement::default()
        });
        Record::Appear {
            slot,
            id,
            name: "",
            appearance: Appearance::default(),
            state,
        }
    }

    #[test]
    fn a_fighter_runs_at_the_nearest_body_not_rooted_and_swings_once_close() {
        let mut sight = Sight::default();
        for record in [
            appear(0, 7, 10.0, 0.0, 0),
            appear(1, 8, 0.0, 3.0, flags::ROOT),
            appear(2, 9, 0.0, -12.0, 0),
        ] {
            sight.see(&record, [0.0; 3]);
        }
        let mut fighter = Fighter::default();
        let toward = Aim::Toward {
            facing: 0.0,
            run_yd: 8.0,
        };
        assert_eq!(fighter.frame(0, [0.0, 0.0], &sight), (Some(toward), false));
        assert_eq!(
            fighter.frame(100, [0.0, 0.0], &sight).0,
            None,
            "it aims four times a second"
        );
        assert!(fighter.frame(300, [7.0, 0.0], &sight).1);
        assert!(
            !fighter.frame(400, [7.0, 0.0], &sight).1,
            "and swings twice a second at most"
        );
        sight.see(&Record::Vanish { slot: 0 }, [0.0; 3]);
        let away = fighter.frame(600, [0.0, 0.0], &sight).0;
        assert!(matches!(away, Some(Aim::Toward { run_yd, .. }) if (run_yd - 10.0).abs() < 1e-4));
        assert_eq!(
            Fighter::default().frame(0, [0.0; 2], &Sight::default()).0,
            Some(Aim::Still)
        );
    }
}
