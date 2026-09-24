use std::time::Instant;

use protocol::{VERSION, Welcome};
use rayon::prelude::*;

use crate::grid::Grid;
use crate::net::Shared;
use crate::replicate::{Built, Observer, Scene, Scratch, View, build};
use crate::rules::Rules;
use crate::stats::{Phase, TickStats};
use crate::world::{Order, Spawn, Stamped, World};

/// Observers one replication task builds for.
const OBSERVERS_PER_TASK: usize = 16;

/// The world and everyone's view of it, advanced one tick at a time.
pub struct Sim {
    world: World,
    grid: Grid,
    observers: Vec<Observer>,
    view: View,
    map: u32,
    tick_ms: u16,
    phases: [Phase; 5],
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
        }
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    /// Runs one tick over `inputs`: admits joins and welcomes them, steps every entity, rebuilds
    /// the grid, builds each observer's batch when `replicate` is set and sends it when it has a
    /// connection, and hashes the world. Call from inside the thread pool the tick should use.
    pub fn tick(
        &mut self,
        inputs: &[Stamped],
        order: Order,
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
        } = self;
        let mut st = TickStats {
            tick: world.tick(),
            ..TickStats::default()
        };
        let mut clock = Instant::now();
        let joined = phases[0].time(|| world.admit(inputs));
        for (conn, slot) in joined {
            let outbox = shared.and_then(|s| s.claim_outbox(conn));
            if let Some(outbox) = &outbox {
                let mut bytes = Vec::new();
                Welcome {
                    version: VERSION,
                    id: slot,
                    map: *map,
                    tick: world.tick(),
                    tick_ms: *tick_ms,
                    spawn: world.bodies()[slot as usize].movement,
                }
                .write(&mut bytes);
                outbox.send(bytes);
            }
            observers.push(Observer::new(slot, outbox));
        }
        st.wall[0] = lap(&mut clock);
        let acts = phases[1].time(|| world.route(inputs, order));
        let stepped = world.step(&acts, &phases[1]);
        st.wall[1] = lap(&mut clock);
        phases[2].time(|| grid.rebuild(world.bodies()));
        st.wall[2] = lap(&mut clock);
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
                            sum.add(build(o, &scene, scratch))
                        })
                    })
                })
                .reduce(Built::default, Built::add)
        } else {
            Built::default()
        };
        st.wall[3] = lap(&mut clock);
        let hash = world.hash(&phases[4]);
        st.wall[4] = lap(&mut clock);
        st.players = world.alive() as u32;
        st.hash = hash;
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
            (st.cpu[p], st.largest[p]) = phase.take();
        }
        world.finish();
        st
    }
}

/// The wall time since `clock`, ns, restarting it.
fn lap(clock: &mut Instant) -> u64 {
    let ns = clock.elapsed().as_nanos() as u64;
    *clock = Instant::now();
    ns
}

#[cfg(test)]
mod tests;
