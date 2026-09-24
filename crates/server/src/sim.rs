use std::time::Instant;

use protocol::{VERSION, Welcome};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::grid::Grid;
use crate::net::Shared;
use crate::replicate::{Built, Observer, Scene, Scratch, View, send_batch};
use crate::rules::Rules;
use crate::stats::{Phase, TickStats};
use crate::world::{InputOrder, Refusal, Spawn, Stamped, World};

const OBSERVERS_PER_TASK: usize = 16;

pub struct Sim {
    world: World,
    grid: Grid,
    observers: Vec<Observer>,
    view: View,
    map: u32,
    tick_ms: u16,
    phases: [Phase; 5],
    refusals: Vec<Refusal>,
}

impl Sim {
    pub fn new(spawns: Vec<Spawn>, rules: Rules, view: View, map: u32, tick_ms: u16) -> Self {
        Self {
            world: World::new(spawns, rules),
            grid: Grid::new(view.radius * 0.5),
            observers: Vec::new(),
            view,
            map,
            tick_ms,
            phases: Default::default(),
            refusals: Vec::new(),
        }
    }

    pub fn keep_refusals(&mut self) {
        self.world.keep_refusals();
    }

    pub fn take_refusals(&mut self) -> Vec<Refusal> {
        std::mem::take(&mut self.refusals)
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Runs one tick on `pool`. Batches are built only when `replicate` is set, and sent only
    /// to connections `shared` holds.
    pub fn tick(
        &mut self,
        pool: &ThreadPool,
        inputs: &[Stamped],
        order: InputOrder,
        shared: Option<&Shared>,
        replicate: bool,
    ) -> TickStats {
        pool.install(|| self.run_tick(inputs, order, shared, replicate))
    }

    fn run_tick(
        &mut self,
        inputs: &[Stamped],
        order: InputOrder,
        shared: Option<&Shared>,
        replicate: bool,
    ) -> TickStats {
        let Self {
            world,
            grid,
            observers,
            view,
            map,
            tick_ms,
            phases,
            refusals,
        } = self;
        let mut st = TickStats {
            tick: world.tick(),
            ..TickStats::default()
        };
        let mut clock = Instant::now();
        for joined in phases[0].time(|| world.admit(inputs)) {
            let outbox = shared.and_then(|s| s.take_outbox(joined.conn));
            if let Some(outbox) = &outbox {
                let mut bytes = Vec::new();
                Welcome {
                    version: VERSION,
                    id: joined.slot,
                    map: *map,
                    tick: world.tick(),
                    tick_ms: *tick_ms,
                    spawn: world.bodies()[joined.slot as usize].movement,
                }
                .write(&mut bytes);
                outbox.send(bytes);
            }
            observers.push(Observer::new(joined.slot, outbox));
        }
        st.wall_ns[0] = lap_ns(&mut clock);
        let acts = phases[1].time(|| world.route(inputs, order));
        let mut stepped = world.step(&acts, &phases[1]);
        refusals.append(&mut stepped.refusals);
        st.wall_ns[1] = lap_ns(&mut clock);
        phases[2].time(|| grid.rebuild(world.bodies()));
        st.wall_ns[2] = lap_ns(&mut clock);
        let bodies = world.bodies();
        observers.retain(|o| bodies[o.slot as usize].alive);
        let built = if replicate {
            let scene = Scene { world, grid, view };
            let phase = &phases[3];
            observers
                .par_chunks_mut(OBSERVERS_PER_TASK)
                .map_init(Scratch::default, |scratch, chunk| {
                    phase.time(|| {
                        chunk.iter_mut().fold(Built::default(), |sum, o| {
                            sum.add(send_batch(o, &scene, scratch))
                        })
                    })
                })
                .reduce(Built::default, Built::add)
        } else {
            Built::default()
        };
        st.wall_ns[3] = lap_ns(&mut clock);
        st.hash = world.hash(&phases[4]);
        st.wall_ns[4] = lap_ns(&mut clock);
        st.players = world.alive() as u32;
        st.claims = stepped.claims;
        st.refused = stepped.refused;
        st.stale = stepped.stale;
        st.appeared = built.appeared;
        st.vanished = built.vanished;
        st.moves = built.moves;
        st.deferred = built.deferred;
        st.corrections = built.corrections;
        st.kicked = built.kicked;
        st.bytes_out = built.bytes;
        for (p, phase) in phases.iter().enumerate() {
            let taken = phase.take();
            st.cpu_ns[p] = taken.cpu_ns;
            st.largest_task_ns[p] = taken.largest_task_ns;
        }
        world.finish();
        st
    }
}

fn lap_ns(clock: &mut Instant) -> u64 {
    let ns = clock.elapsed().as_nanos() as u64;
    *clock = Instant::now();
    ns
}

#[cfg(test)]
mod tests;
