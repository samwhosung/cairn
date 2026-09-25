use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::hosted::{BodyOrder, Delivery, Hosted, Took, Turn};
use crate::out::{ACT, Out, ROUND, Spawned};
use crate::record::Record;
use crate::space::Space;
use crate::table::{Pair, Rows};
use crate::{Game, Id, Kind, Kinds, Letter, Spot, Table, Tick, World, canon};

pub(crate) const NEVER: Tick = Tick::MAX;
const ROUNDS: u8 = 2;

pub(crate) struct Clock {
    cpu: fn() -> u64,
    cpu_ns: AtomicU64,
    largest_ns: AtomicU64,
}

impl Clock {
    fn new(cpu: fn() -> u64) -> Self {
        Self {
            cpu,
            cpu_ns: AtomicU64::new(0),
            largest_ns: AtomicU64::new(0),
        }
    }

    pub fn time<R>(&self, f: impl FnOnce() -> R) -> R {
        let started = (self.cpu)();
        let out = f();
        let ns = (self.cpu)().saturating_sub(started);
        self.cpu_ns.fetch_add(ns, Ordering::Relaxed);
        self.largest_ns.fetch_max(ns, Ordering::Relaxed);
        out
    }

    fn took(&self, wall_ns: u64) -> Took {
        Took {
            cpu_ns: self.cpu_ns.load(Ordering::Relaxed),
            largest_ns: self.largest_ns.load(Ordering::Relaxed),
            wall_ns,
        }
    }
}

/// A game's rows, and the tick that runs its rules on them.
pub struct Engine<G: Game> {
    knobs: G::Knobs,
    seed: u64,
    tick_ms: u32,
    delivery: Delivery,
    tick: Tick,
    kinds: Vec<TypeId>,
    prevs: Vec<Box<dyn Any + Send + Sync>>,
    lives: Vec<Box<dyn Rows<G>>>,
    next_n: Vec<u32>,
    space: Space,
    carry: Vec<(Id, Letter<G::Msg>)>,
    record: Record,
    shown: Vec<(Vec<u8>, Tick)>,
    orders: Vec<(u32, BodyOrder)>,
    counts: BTreeMap<&'static str, i64>,
    took: [Took; 3],
}

/// Where a tick's outs go.
struct Gathered<G: Game> {
    mail: Vec<(Id, Letter<G::Msg>)>,
    spawns: Vec<Spawned>,
    orders: Vec<(u32, u8, BodyOrder)>,
}

impl<G: Game> Gathered<G> {
    fn take(&mut self, outs: Vec<Out<G>>, counts: &mut BTreeMap<&'static str, i64>) {
        for mut out in outs {
            self.mail.append(&mut out.letters);
            self.spawns.append(&mut out.spawns);
            self.orders.append(&mut out.orders);
            for (what, by) in out.counts.drain(..) {
                debug_assert!(
                    G::COUNTS.contains(&what),
                    "{} counts `{what}` undeclared",
                    G::NAME
                );
                *counts.entry(what).or_default() += by;
            }
        }
    }
}

impl<G: Game> Engine<G> {
    /// # Panics
    /// If the game declares more kinds than a row's name can count.
    pub fn new(knobs: G::Knobs, seed: u64, tick_ms: u32, delivery: Delivery) -> Self {
        let mut declared = Kinds {
            tables: vec![Pair::of::<G::Player>()],
        };
        G::kinds(&mut declared);
        assert!(
            u16::try_from(declared.tables.len()).is_ok(),
            "too many kinds"
        );
        let (mut kinds, mut prevs, mut lives) = (Vec::new(), Vec::new(), Vec::new());
        for pair in declared.tables {
            kinds.push(pair.kind);
            prevs.push(pair.prev);
            lives.push(pair.live);
        }
        Self {
            knobs,
            seed,
            tick_ms,
            delivery,
            tick: 0,
            next_n: vec![0; kinds.len()],
            kinds,
            prevs,
            lives,
            space: Space::default(),
            carry: Vec::new(),
            record: Record::default(),
            shown: Vec::new(),
            orders: Vec::new(),
            counts: G::COUNTS.iter().map(|&what| (what, 0)).collect(),
            took: [Took::default(); 3],
        }
    }

    /// The world as the last tick left it.
    pub fn world(&self) -> World<'_, G> {
        World {
            tick: self.tick,
            tick_ms: self.tick_ms,
            seed: self.seed,
            knobs: &self.knobs,
            tables: &self.prevs,
            kinds: &self.kinds,
            space: &self.space,
        }
    }

    pub fn table<K: Kind<G>>(&self) -> Option<&Table<K>> {
        self.world().table()
    }

    fn join(&mut self, n: u32, spawn: Spot) {
        assert_eq!(
            n, self.next_n[0],
            "players join in the order of their bodies"
        );
        self.space.join(n, spawn);
        let id = Id::player(n);
        let row = G::join(id, &self.world());
        self.next_n[0] = n + 1;
        let (sent, saved) = self.lives[0].push(self.prevs[0].as_mut(), n, Box::new(row), self.tick);
        self.record.came.push(id);
        self.record.shown.push((id, sent));
        self.record.saved.push((id, Some(saved)));
    }

    fn spawn(&mut self, mut spawns: Vec<Spawned>) {
        spawns.sort_by_key(|s| (s.phase, s.from, s.seq));
        for s in spawns {
            let Some(k) = self
                .kinds
                .iter()
                .position(|&t| t == s.kind)
                .filter(|&k| k > 0)
            else {
                panic!("{} spawned a row of no kind it declared", G::NAME);
            };
            let n = self.next_n[k];
            self.next_n[k] += 1;
            let (sent, saved) = self.lives[k].push(self.prevs[k].as_mut(), n, s.row, self.tick + 1);
            let id = Id { kind: k as u16, n };
            self.record.came.push(id);
            self.record.shown.push((id, sent));
            self.record.saved.push((id, Some(saved)));
        }
    }

    fn settle_orders(&mut self, mut orders: Vec<(u32, u8, BodyOrder)>) {
        orders.sort_by_key(|&(n, phase, _)| (n, phase));
        self.orders.clear();
        for (n, _, o) in orders {
            match self.orders.last_mut() {
                Some((m, all)) if *m == n => {
                    all.place = o.place.or(all.place);
                    all.root = o.root.or(all.root);
                }
                _ => self.orders.push((n, o)),
            }
        }
    }
}

/// Reverses the letters of each run of one target in `to`, which is sorted.
fn reverse_each_run<T: PartialEq, L>(to: &[T], letters: &mut [L]) {
    let mut i = 0;
    while i < to.len() {
        let end = i + to[i..].partition_point(|t| *t == to[i]);
        letters[i..end].reverse();
        i = end;
    }
}

impl<G: Game> Hosted for Engine<G> {
    fn name(&self) -> &'static str {
        G::NAME
    }

    fn tick(&mut self, turn: &Turn<'_>) {
        let clocks = [(); 3].map(|()| Clock::new(turn.cpu_ns));
        let mut walls = [Instant::now(); 4];
        self.tick = turn.tick;
        self.record.open(turn.tick);
        clocks[0].time(|| {
            self.space.update(turn.bodies);
            for &(n, spawn) in turn.joined {
                self.join(n, spawn);
            }
        });
        let mut got = Gathered::<G> {
            mail: std::mem::take(&mut self.carry),
            spawns: Vec::new(),
            orders: Vec::new(),
        };
        let mut acts: Vec<(u32, Letter<G::Msg>)> = turn
            .actions
            .iter()
            .filter_map(|&(n, number)| {
                let msg = G::action(number)?;
                let from = Id::player(n);
                Some((n, Letter { from, msg }))
            })
            .collect();
        acts.sort_by_key(|a| a.0);
        let (targets, mut letters): (Vec<u32>, Vec<Letter<G::Msg>>) = acts.into_iter().unzip();
        if self.delivery == Delivery::Reversed {
            reverse_each_run(&targets, &mut letters);
        }
        let world = World {
            tick: self.tick,
            tick_ms: self.tick_ms,
            seed: self.seed,
            knobs: &self.knobs,
            tables: &self.prevs,
            kinds: &self.kinds,
            space: &self.space,
        };
        let outs = self.lives[0].apply(0, ACT, &targets, &letters, &world, &clocks[0]);
        got.take(outs, &mut self.counts);
        for (k, live) in self.lives.iter_mut().enumerate() {
            got.take(live.step(k as u16, &world, &clocks[0]), &mut self.counts);
        }
        walls[1] = Instant::now();
        for round in 0..ROUNDS {
            if got.mail.is_empty() {
                break;
            }
            clocks[1].time(|| got.mail.sort_unstable());
            let (to, mut letters): (Vec<Id>, Vec<Letter<G::Msg>>) = got.mail.drain(..).unzip();
            if self.delivery == Delivery::Reversed {
                reverse_each_run(&to, &mut letters);
            }
            for (k, live) in self.lives.iter_mut().enumerate() {
                let kind = k as u16;
                let (a, b) = (
                    to.partition_point(|t| t.kind < kind),
                    to.partition_point(|t| t.kind <= kind),
                );
                if a == b {
                    continue;
                }
                let targets: Vec<u32> = to[a..b].iter().map(|t| t.n).collect();
                let phase = ROUND + round;
                let outs = live.apply(kind, phase, &targets, &letters[a..b], &world, &clocks[1]);
                got.take(outs, &mut self.counts);
            }
        }
        self.carry = std::mem::take(&mut got.mail);
        walls[2] = Instant::now();
        for (k, live) in self.lives.iter_mut().enumerate() {
            live.finish(
                self.prevs[k].as_mut(),
                k as u16,
                &mut self.record,
                &clocks[2],
            );
        }
        clocks[2].time(|| {
            self.spawn(std::mem::take(&mut got.spawns));
            self.record.close();
            for (id, bytes) in &self.record.shown {
                if !id.is_player() {
                    continue;
                }
                let n = id.n as usize;
                if self.shown.len() <= n {
                    self.shown.resize(n + 1, (Vec::new(), 0));
                }
                self.shown[n] = (bytes.clone(), self.tick);
            }
        });
        self.settle_orders(got.orders);
        walls[3] = Instant::now();
        let mut stage = 0;
        self.took = clocks.map(|c| {
            stage += 1;
            c.took(walls[stage].duration_since(walls[stage - 1]).as_nanos() as u64)
        });
    }

    fn orders(&self) -> &[(u32, BodyOrder)] {
        &self.orders
    }

    fn record(&self) -> &Record {
        &self.record
    }

    fn shown(&self, n: u32) -> Option<(&[u8], Tick)> {
        self.shown
            .get(n as usize)
            .map(|(bytes, at)| (bytes.as_slice(), *at))
    }

    fn hash(&self) -> u64 {
        let kinds: Vec<u64> = (0..self.lives.len())
            .map(|k| self.lives[k].hash(k as u16))
            .collect();
        canon::hash(&(kinds, &self.next_n, &self.carry))
    }

    fn saved(&self) -> BTreeMap<Id, Vec<u8>> {
        let mut all = BTreeMap::new();
        for (k, live) in self.lives.iter().enumerate() {
            live.saved(k as u16, &mut all);
        }
        all
    }

    fn counts(&self) -> &BTreeMap<&'static str, i64> {
        &self.counts
    }

    fn took(&self) -> [Took; 3] {
        self.took
    }
}
