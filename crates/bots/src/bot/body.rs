use protocol::Movement;
use server::Spawn;

use crate::fight::{Fighter, Sight, Swing};
use crate::ground::Ground;
use crate::mover::{Claims, Frame, Mover};
use crate::track::{Gait, Pace, RUN, Track};

const FIGHTING: Pace = Pace {
    speed: RUN,
    gait: Gait::Run,
    stop_yd: None,
    jump_every_ms: None,
    surface: false,
};

#[derive(Clone, Copy)]
pub enum Told {
    Corrected(u32),
    Placed { seq: u32, rooted: bool, at: Spawn },
}

pub struct Body {
    pub mover: Mover,
    track: Track,
    fighter: Option<Fighter>,
    until_ms: u32,
}

impl Body {
    pub fn walking(mover: Mover, track: Track, until_ms: u32) -> Self {
        Self {
            mover,
            track,
            fighter: None,
            until_ms,
        }
    }

    pub fn fighting(spawn: &Spawn, now: u32, until_ms: u32) -> Self {
        Self {
            mover: Mover::new(spawn.pos, spawn.facing, Vec::new(), Claims::ByCadence),
            track: standing(spawn, now, until_ms),
            fighter: Some(Fighter::default()),
            until_ms,
        }
    }

    pub fn take(&mut self, told: Told, now: u32) {
        match told {
            Told::Corrected(seq) => self.mover.correct(seq),
            Told::Placed { seq, rooted, at } => {
                self.mover.correct(seq);
                if self.fighter.is_some() {
                    self.mover.rooted = rooted;
                    self.mover.land(at.pos[2]);
                    self.track = standing(&at, now, self.until_ms);
                }
            }
        }
    }

    pub fn frame(
        &mut self,
        t: u32,
        sight: Option<&Sight>,
        ground: &Ground,
        out: &mut Vec<Movement>,
    ) -> (Frame, Option<Swing>) {
        let rooted = self.mover.rooted;
        let swing = match (&mut self.fighter, sight) {
            (Some(fighter), Some(sight)) => {
                fighter.steer(t, rooted, &mut self.track, &FIGHTING, self.until_ms, sight)
            }
            _ => None,
        };
        (self.mover.frame(t, &self.track, ground, out), swing)
    }
}

fn standing(at: &Spawn, from_ms: u32, until_ms: u32) -> Track {
    let stand = Pace {
        stop_yd: Some(0.0),
        ..FIGHTING
    };
    Track::line(at, &stand, from_ms, until_ms.max(from_ms))
}

#[cfg(test)]
mod tests {
    use protocol::{Appearance, Record, State, flags};

    use super::*;
    use crate::bot::{FRAME_MS, dist, ground};

    fn at(x: f32) -> Spawn {
        Spawn {
            pos: [x, 0.0, 0.0],
            facing: 0.0,
        }
    }

    fn claims(body: &mut Body, sight: &Sight, from_ms: u32, to_ms: u32) -> Vec<Movement> {
        let (ground, mut out) = (Ground::flat(0.0), Vec::new());
        for t in (from_ms..=to_ms).step_by(FRAME_MS as usize) {
            body.frame(t, Some(sight), &ground, &mut out);
        }
        out
    }

    #[test]
    fn a_fighter_stands_where_a_game_roots_it_and_fights_on_from_where_it_is_freed() {
        let mut sight = Sight::default();
        let state = State::of(&Movement {
            pos: [20.0, 0.0, 0.0],
            ..Movement::default()
        });
        let other = Record::Appear {
            slot: 0,
            id: 9,
            name: "",
            appearance: Appearance::default(),
            state,
        };
        sight.see(&other, [0.0; 3]);
        let mut body = Body::fighting(&at(0.0), 0, 60_000);
        let ran = claims(&mut body, &sight, 0, 1000);
        assert!(ran.last().is_some_and(|m| m.pos[0] > 5.0), "{ran:?}");
        let rooted = Told::Placed {
            seq: 1,
            rooted: true,
            at: at(3.0),
        };
        body.take(rooted, 1000);
        let dead = claims(&mut body, &sight, 1050, 5000);
        let still =
            |m: &Movement| dist(ground(m.pos), [3.0, 0.0]) < 1e-6 && m.flags & flags::ROOT != 0;
        assert!(!dead.is_empty() && dead.iter().all(still), "{dead:?}");
        assert_eq!(body.mover.ack, 1);
        let freed = Told::Placed {
            seq: 2,
            rooted: false,
            at: at(-10.0),
        };
        body.take(freed, 5000);
        let risen = claims(&mut body, &sight, 5050, 6050);
        assert!(
            risen
                .first()
                .is_some_and(|m| dist(ground(m.pos), [-10.0, 0.0]) < 1e-6),
            "{risen:?}"
        );
        let on = |m: &Movement| m.pos[0] > -5.0 && m.flags & flags::ROOT == 0;
        assert!(risen.last().is_some_and(on), "{risen:?}");
        assert_eq!(body.mover.ack, 2);
    }
}
