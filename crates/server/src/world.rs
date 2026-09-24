use std::collections::HashMap;
use std::sync::Mutex;

use protocol::{Appearance, Claim, Hello, Movement};
use rayon::prelude::*;

use crate::rules::{Rules, Verdict};
use crate::stats::Phase;

/// Entities stepped per task.
const CHUNK: usize = 256;
/// Inputs per task when the control scrambles their order.
const RACY_CHUNK: usize = 4;

/// Where a joining player first stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub pos: [f32; 3],
    pub facing: f32,
}

/// What a connection asked of the world, stamped with the connection, its order on that
/// connection, and when the server received it.
#[derive(Clone, Debug)]
pub struct Stamped {
    pub conn: u32,
    pub seq: u32,
    pub at_ms: u32,
    pub input: Input,
}

#[derive(Clone, Debug)]
pub enum Input {
    Join(Hello),
    Claim(Claim),
    Leave,
}

/// The order a tick applies each entity's inputs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// As each connection sent them.
    Canonical,
    /// As worker threads happened to hand them over: the control that must break determinism.
    Racy,
}

/// One entity's simulated state: the last movement the server accepted and what judging its
/// claims needs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    pub movement: Movement,
    pub alive: bool,
    /// Whether a claim has been accepted, which pins the client's clock to the server's.
    pub clocked: bool,
    pub clock_time: u32,
    pub clock_at: u32,
    /// The latest correction; claims count again once they acknowledge it.
    pub seq: u32,
    /// The tick of the latest refusal, `u32::MAX` before any.
    pub corrected: u32,
    /// The tick the movement last changed.
    pub changed: u32,
    /// The tick the movement flags last changed.
    pub turned: u32,
    pub refused: u32,
    pub stale: u32,
}

/// One of an entity's inputs for this tick, routed to its slot.
#[derive(Clone, Copy, Debug)]
pub enum Act {
    Claim { slot: u32, at_ms: u32, claim: Claim },
    Leave { slot: u32 },
}

impl Act {
    pub fn slot(&self) -> u32 {
        match *self {
            Self::Claim { slot, .. } | Self::Leave { slot } => slot,
        }
    }
}

/// What one tick's step did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stepped {
    pub claims: u32,
    pub refused: u32,
    pub stale: u32,
}

/// The world: plain arrays indexed by slot, which is also the entity's id. `prev` is the last
/// tick's world, read by every rule; `next` is this tick's, each slot written only by its own
/// entity's step.
pub struct World {
    tick: u32,
    prev: Vec<Body>,
    next: Vec<Body>,
    names: Vec<String>,
    looks: Vec<Appearance>,
    slot_of: HashMap<u32, u32>,
    spawns: Vec<Spawn>,
    rules: Rules,
}

impl World {
    pub fn new(spawns: Vec<Spawn>, rules: Rules) -> Self {
        let spawns = if spawns.is_empty() {
            vec![Spawn {
                pos: [0.0; 3],
                facing: 0.0,
            }]
        } else {
            spawns
        };
        Self {
            tick: 0,
            prev: Vec::new(),
            next: Vec::new(),
            names: Vec::new(),
            looks: Vec::new(),
            slot_of: HashMap::new(),
            spawns,
            rules,
        }
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn spawns(&self) -> &[Spawn] {
        &self.spawns
    }

    /// This tick's world, once stepped.
    pub fn bodies(&self) -> &[Body] {
        &self.next
    }

    pub fn name(&self, slot: u32) -> &str {
        &self.names[slot as usize]
    }

    pub fn look(&self, slot: u32) -> &Appearance {
        &self.looks[slot as usize]
    }

    pub fn alive(&self) -> usize {
        self.next.iter().filter(|b| b.alive).count()
    }

    /// Gives every join among `inputs` a slot at its spawn, in input order, and returns the
    /// connections and slots it gave.
    pub fn admit(&mut self, inputs: &[Stamped]) -> Vec<(u32, u32)> {
        let mut joined = Vec::new();
        for s in inputs {
            let Input::Join(hello) = &s.input else {
                continue;
            };
            if self.slot_of.contains_key(&s.conn) {
                continue;
            }
            let slot = self.prev.len() as u32;
            let spawn = self.spawns[slot as usize % self.spawns.len()];
            let body = Body {
                movement: Movement {
                    pos: spawn.pos,
                    facing: spawn.facing,
                    ..Movement::default()
                },
                alive: true,
                corrected: u32::MAX,
                changed: self.tick,
                turned: self.tick,
                ..Body::default()
            };
            self.prev.push(body);
            self.next.push(body);
            self.names.push(hello.name.clone());
            self.looks.push(hello.appearance);
            self.slot_of.insert(s.conn, slot);
            joined.push((s.conn, slot));
        }
        joined
    }

    /// The claims and leaves among `inputs`, routed to their slots and sorted by slot: in each
    /// connection's own order, or in `Order::Racy` as the worker threads hand them over.
    pub fn route(&self, inputs: &[Stamped], order: Order) -> Vec<Act> {
        let acts: Vec<Act> = inputs
            .iter()
            .filter_map(|s| {
                let slot = *self.slot_of.get(&s.conn)?;
                match &s.input {
                    Input::Join(_) => None,
                    Input::Claim(claim) => Some(Act::Claim {
                        slot,
                        at_ms: s.at_ms,
                        claim: *claim,
                    }),
                    Input::Leave => Some(Act::Leave { slot }),
                }
            })
            .collect();
        let mut acts = match order {
            Order::Canonical => acts,
            Order::Racy => {
                let arrived = Mutex::new(Vec::with_capacity(acts.len()));
                acts.par_chunks(RACY_CHUNK).for_each(|c| {
                    if let Ok(mut a) = arrived.lock() {
                        a.extend(c.iter().rev());
                    }
                });
                arrived.into_inner().unwrap_or_default()
            }
        };
        acts.sort_by_key(Act::slot);
        acts
    }

    /// Steps every entity from `prev` into `next`, in parallel: each applies its own acts, in
    /// the order `acts` holds them, and nothing else.
    pub fn step(&mut self, acts: &[Act], phase: &Phase) -> Stepped {
        let prev = &self.prev;
        let rules = &self.rules;
        let tick = self.tick;
        self.next
            .par_chunks_mut(CHUNK)
            .enumerate()
            .map(|(ci, chunk)| {
                phase.time(|| {
                    let lo = (ci * CHUNK) as u32;
                    let hi = lo + chunk.len() as u32;
                    chunk.copy_from_slice(&prev[lo as usize..hi as usize]);
                    let a = acts.partition_point(|a| a.slot() < lo);
                    let b = acts.partition_point(|a| a.slot() < hi);
                    let mut done = Stepped::default();
                    for act in &acts[a..b] {
                        let body = &mut chunk[(act.slot() - lo) as usize];
                        apply(body, act, tick, rules, &mut done);
                    }
                    done
                })
            })
            .reduce(Stepped::default, |x, y| Stepped {
                claims: x.claims + y.claims,
                refused: x.refused + y.refused,
                stale: x.stale + y.stale,
            })
    }

    /// A hash of every entity's simulated state, the same whatever the thread count.
    pub fn hash(&self, phase: &Phase) -> u64 {
        self.next
            .par_chunks(CHUNK)
            .enumerate()
            .map(|(ci, chunk)| {
                phase.time(|| {
                    chunk.iter().enumerate().fold(0u64, |h, (j, b)| {
                        h.wrapping_add(hash_body((ci * CHUNK + j) as u32, b))
                    })
                })
            })
            .reduce(|| 0, u64::wrapping_add)
    }

    /// Ends the tick: this tick's world becomes the snapshot the next one reads.
    pub fn finish(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
        self.tick += 1;
    }
}

fn apply(body: &mut Body, act: &Act, tick: u32, rules: &Rules, done: &mut Stepped) {
    if !body.alive {
        return;
    }
    let (at_ms, claim) = match *act {
        Act::Claim { at_ms, claim, .. } => (at_ms, claim),
        Act::Leave { .. } => {
            body.alive = false;
            body.changed = tick;
            return;
        }
    };
    done.claims += 1;
    match rules.judge(body, &claim, at_ms) {
        Verdict::Accept => {
            let m = claim.movement;
            if !body.clocked {
                body.clocked = true;
                body.clock_time = m.time;
                body.clock_at = at_ms;
            }
            if m.flags != body.movement.flags {
                body.turned = tick;
            }
            body.movement = m;
            body.changed = tick;
        }
        Verdict::Stale => {
            body.stale += 1;
            done.stale += 1;
        }
        Verdict::Refuse(_) => {
            body.seq = body.seq.wrapping_add(1);
            body.corrected = tick;
            body.refused += 1;
            done.refused += 1;
        }
    }
}

fn hash_body(slot: u32, b: &Body) -> u64 {
    let m = &b.movement;
    let words = [
        slot,
        u32::from(b.alive),
        m.time,
        m.flags,
        m.pos[0].to_bits(),
        m.pos[1].to_bits(),
        m.pos[2].to_bits(),
        m.facing.to_bits(),
        m.pitch.to_bits(),
        m.fall_time,
        m.jump.z_speed.to_bits(),
        m.jump.cos.to_bits(),
        m.jump.sin.to_bits(),
        m.jump.xy_speed.to_bits(),
        u32::from(b.clocked),
        b.clock_time,
        b.clock_at,
        b.seq,
        b.corrected,
        b.changed,
        b.turned,
        b.refused,
        b.stale,
    ];
    words.iter().fold(0xcbf2_9ce4_8422_2325, |h, &w| {
        (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3)
    })
}
