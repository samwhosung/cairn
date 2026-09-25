use std::time::Instant;

use game::{Hosted, Spot, Stages, Turn};
use protocol::{Movement, VERSION, Welcome};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::grid::Grid;
use crate::limits::Limits;
use crate::net::{Held, Shared};
use crate::relays::Relays;
use crate::replicate::{Built, Observer, Scene, Scratch, View, send_batch};
use crate::save::{Keeping, Roster, Writer};
use crate::stats::{PHASES, Phase, TickStats, thread_cpu_ns};
use crate::world::{Act, InputOrder, Refusal, Spawn, Stamped, World};

const OBSERVERS_PER_TASK: usize = 16;
const ADMIT: usize = 0;
const STEP: usize = 1;
const RULES: usize = 2;
const DELIVER: usize = 3;
const RECORD: usize = 4;
const INDEX: usize = 5;
const ENCODE: usize = 6;
const REPLICATE: usize = 7;
const HASH: usize = 8;

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
    relays: Relays,
    observers: Vec<Observer>,
    view: View,
    map: u32,
    tick_ms: u16,
    phases: [Phase; PHASES.len()],
    refusals: Vec<Refusal>,
    game: Option<Box<dyn Hosted>>,
    roster: Roster,
    writer: Option<Writer>,
    held: Vec<Held>,
    bodies: Vec<Option<Spot>>,
}

impl Sim {
    pub fn new(spawns: Vec<Spawn>, limits: Limits, view: View, map: u32, tick_ms: u16) -> Self {
        Self {
            world: World::new(spawns, limits),
            grid: Grid::new(view.radius * 0.5),
            relays: Relays::default(),
            observers: Vec::new(),
            view,
            map,
            tick_ms,
            phases: Default::default(),
            refusals: Vec::new(),
            game: None,
            roster: Roster::new(Vec::new(), map, tick_ms),
            writer: None,
            held: Vec::new(),
            bodies: Vec::new(),
        }
    }

    pub fn with_game(mut self, game: Option<Box<dyn Hosted>>) -> Self {
        self.game = game;
        self
    }

    pub fn with_roster(mut self, roster: Roster, writer: Option<Writer>) -> Self {
        self.roster = roster;
        self.writer = writer;
        self
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

    pub fn game(&self) -> Option<&dyn Hosted> {
        self.game.as_deref()
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub fn writer(&self) -> Option<&Writer> {
        self.writer.as_ref()
    }

    pub fn take_writer(&mut self) -> Option<Writer> {
        self.writer.take()
    }

    pub fn keeping(&self) -> Keeping {
        let schema = self.game.as_ref().and_then(|g| g.tables().players);
        let scan = self.game.as_ref().map(|g| g.saved()).unwrap_or_default();
        self.roster.keeping(schema.as_ref(), &scan)
    }

    pub fn release(&mut self) {
        if let Some(writer) = &self.writer {
            let tick = self.world.tick().wrapping_sub(1);
            writer.release(tick, std::mem::take(&mut self.held));
        }
    }

    pub fn stop(&mut self) {
        self.roster.save_every_place(self.world.before());
        let batch = self.roster.take_batch(self.world.tick());
        if let Some(writer) = &self.writer {
            writer.save(batch);
        }
    }
    pub fn in_view(&self, id: u32) -> Option<Vec<(u16, u32)>> {
        self.observers
            .iter()
            .find(|o| o.id == id)
            .map(Observer::in_view)
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

    #[allow(clippy::too_many_lines)]
    fn run_tick(
        &mut self,
        inputs: &[Stamped],
        order: InputOrder,
        batches: Batches<'_>,
    ) -> TickStats {
        let Self {
            world,
            grid,
            relays,
            observers,
            view,
            map,
            tick_ms,
            phases,
            refusals,
            game,
            roster,
            writer,
            held,
            bodies,
        } = self;
        let mut st = TickStats {
            tick: world.tick(),
            ..TickStats::default()
        };
        let mut clock = Instant::now();
        let (mut joined, mut restored) = (Vec::new(), Vec::new());
        let admission = phases[ADMIT].time(|| world.admit(inputs, |n| roster.admit(n)));
        if let Batches::Send(shared) = batches {
            for &conn in &admission.refused {
                drop(shared.take_outbox(conn));
            }
        }
        let welcome = |conn: u32, id: u32, spawn: Movement| {
            let outbox = match batches {
                Batches::Send(shared) => shared.take_outbox(conn),
                Batches::Skip => None,
            };
            if let Some(outbox) = &outbox {
                let mut bytes = Vec::new();
                Welcome {
                    version: VERSION,
                    id,
                    map: *map,
                    tick: admission.tick,
                    tick_ms: *tick_ms,
                    spawn,
                }
                .write(&mut bytes);
                outbox.send(bytes);
            }
            Observer::new(id, conn, outbox)
        };
        for admitted in &admission.admitted {
            let name = world.name(admitted.id);
            roster.bind(admitted.id, name);
            if let Some(saved) = roster.saved(name) {
                restored.push((admitted.id, saved.to_vec()));
            }
            observers.push(welcome(admitted.conn, admitted.id, admitted.spawn));
            let spawn = admitted.spawn;
            joined.push((
                admitted.id,
                Spot {
                    pos: spawn.pos,
                    facing: spawn.facing,
                },
            ));
        }
        for taken in &admission.taken_over {
            let new = welcome(taken.conn, taken.id, taken.spawn);
            match observers.iter_mut().find(|o| o.id == taken.id) {
                Some(had) => *had = new,
                None => observers.push(new),
            }
        }
        st.wall_ns[ADMIT] = lap_ns(&mut clock);
        let acts = phases[STEP].time(|| world.route(inputs, order));
        let mut stepped = world.step(&acts, &phases[STEP]);
        refusals.append(&mut stepped.refusals);
        for act in &acts {
            let Act::Leave { id } = *act else { continue };
            let (was, now) = (&world.before()[id as usize], &world.stepped()[id as usize]);
            if was.present && !now.present {
                roster.leave(id, now);
            }
        }
        st.wall_ns[STEP] = lap_ns(&mut clock);
        if let Some(game) = game {
            let actions = phases[RULES].time(|| {
                bodies.clear();
                bodies.extend(world.before().iter().map(|b| {
                    b.present.then_some(Spot {
                        pos: b.movement.pos,
                        facing: b.movement.facing,
                    })
                }));
                world.actions(inputs)
            });
            game.tick(&Turn {
                tick: world.tick(),
                joined: &joined,
                restored: &restored,
                bodies,
                actions: &actions,
                cpu_ns: thread_cpu_ns,
            });
            let playing = lap_ns(&mut clock);
            phases[RECORD].time(|| {
                world.order(game.orders());
                roster.take_player_rows(game.record());
            });
            let Stages {
                rules,
                deliver,
                record,
            } = game.took();
            st.wall_ns[RULES] = playing.saturating_sub(deliver.wall_ns + record.wall_ns);
            st.wall_ns[DELIVER] = deliver.wall_ns;
            st.wall_ns[RECORD] = record.wall_ns + lap_ns(&mut clock);
            for (p, took) in [(RULES, rules), (DELIVER, deliver), (RECORD, record)] {
                st.cpu_ns[p] = took.cpu_ns;
                st.largest_task_ns[p] = took.largest_ns;
            }
        }
        let batch = phases[RECORD].time(|| {
            roster.save_due_places(world.tick(), world.stepped());
            roster.take_batch(world.tick())
        });
        st.saved_rows = batch.rows() as u32;
        if let Some(writer) = writer {
            writer.save(batch);
        }
        phases[INDEX].time(|| grid.rebuild(world.stepped()));
        st.wall_ns[INDEX] = lap_ns(&mut clock);
        let sending = matches!(batches, Batches::Send(_));
        if sending {
            relays.update(world, &phases[ENCODE]);
        }
        st.wall_ns[ENCODE] = lap_ns(&mut clock);
        let present = world.stepped();
        observers.retain(|o| present[o.id as usize].present);
        let built = if let Batches::Send(clients) = batches {
            let scene = Scene {
                world,
                grid,
                view,
                relays,
                clients,
                game: game.as_deref(),
                hold_until_committed: writer.is_some(),
            };
            let phase = &phases[REPLICATE];
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
        held.extend(observers.iter_mut().filter_map(|o| o.held.take()));
        st.wall_ns[REPLICATE] = lap_ns(&mut clock);
        st.hash = world.hash(&phases[HASH]);
        if let Some(game) = game {
            st.hash ^= phases[HASH].time(|| game.hash()).rotate_left(32);
        }
        st.wall_ns[HASH] = lap_ns(&mut clock);
        st.players = world.present() as u32;
        st.claims = stepped.claims;
        st.refused = stepped.refused;
        st.stale = stepped.stale;
        st.built = built;
        for (p, phase) in phases.iter().enumerate() {
            let taken = phase.take();
            st.cpu_ns[p] += taken.cpu_ns;
            st.largest_task_ns[p] = st.largest_task_ns[p].max(taken.largest_task_ns);
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
