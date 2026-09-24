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

/// What a tick does with its observers' batches.
#[derive(Clone, Copy)]
pub enum Batches<'a> {
    Skip,
    /// Build them, and send each to its connection when `Shared` held it as the player joined.
    Send(&'a Shared),
}

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

    pub fn tick(
        &mut self,
        pool: &ThreadPool,
        inputs: &[Stamped],
        order: InputOrder,
        batches: Batches<'_>,
    ) -> TickStats {
        pool.install(|| self.run_tick(inputs, order, batches))
    }

    fn run_tick(
        &mut self,
        inputs: &[Stamped],
        order: InputOrder,
        batches: Batches<'_>,
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
            let outbox = match batches {
                Batches::Send(shared) => shared.take_outbox(joined.conn),
                Batches::Skip => None,
            };
            if let Some(outbox) = &outbox {
                let mut bytes = Vec::new();
                Welcome {
                    version: VERSION,
                    id: joined.id,
                    map: *map,
                    tick: world.tick(),
                    tick_ms: *tick_ms,
                    spawn: joined.spawn,
                }
                .write(&mut bytes);
                outbox.send(bytes);
            }
            observers.push(Observer::new(joined.id, outbox));
        }
        st.wall_ns[0] = lap_ns(&mut clock);
        let acts = phases[1].time(|| world.route(inputs, order));
        let mut stepped = world.step(&acts, &phases[1]);
        refusals.append(&mut stepped.refusals);
        st.wall_ns[1] = lap_ns(&mut clock);
        phases[2].time(|| grid.rebuild(world.stepped()));
        st.wall_ns[2] = lap_ns(&mut clock);
        let bodies = world.stepped();
        observers.retain(|o| bodies[o.id as usize].alive);
        let built = if matches!(batches, Batches::Send(_)) {
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
