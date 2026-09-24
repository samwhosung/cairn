use protocol::{Intro, Relay};
use rayon::prelude::*;

use crate::stats::Phase;
use crate::world::World;

const ENTITIES_PER_TASK: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
pub struct Hot {
    pub xy: [f32; 2],
    pub alive: bool,
    pub state_changed_at: u32,
    pub pos_changed_at: u32,
    pub facing_changed_at: u32,
}

#[derive(Default)]
pub struct Relays {
    pub hot: Vec<Hot>,
    pub pieces: Vec<Relay>,
    pub intros: Vec<Intro>,
}

impl Relays {
    pub fn update(&mut self, world: &World, phase: &Phase) {
        let bodies = world.stepped();
        for id in self.intros.len() as u32..bodies.len() as u32 {
            self.intros
                .push(Intro::new(id, world.name(id), world.look(id)));
            self.pieces.push(Relay::default());
            self.hot.push(Hot::default());
        }
        let tick = world.tick();
        self.hot
            .par_chunks_mut(ENTITIES_PER_TASK)
            .zip(self.pieces.par_chunks_mut(ENTITIES_PER_TASK))
            .zip(bodies.par_chunks(ENTITIES_PER_TASK))
            .for_each(|((hot, pieces), bodies)| {
                phase.time(|| {
                    for ((h, r), b) in hot.iter_mut().zip(pieces).zip(bodies) {
                        if b.moved_at != tick {
                            continue;
                        }
                        let fresh = Relay::of(&b.movement);
                        let changed = fresh.changed_from(r);
                        for (at, yes) in [
                            (&mut h.state_changed_at, changed.state),
                            (&mut h.pos_changed_at, changed.pos),
                            (&mut h.facing_changed_at, changed.facing),
                        ] {
                            if yes {
                                *at = tick;
                            }
                        }
                        *r = fresh;
                        h.xy = [b.movement.pos[0], b.movement.pos[1]];
                        h.alive = b.alive;
                    }
                });
            });
    }
}
