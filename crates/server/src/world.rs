use std::collections::HashMap;
use std::sync::Mutex;

use game::BodyOrder;
use protocol::{Appearance, Claim, Hello, Movement, flags};
use rayon::prelude::*;

use crate::limits::{ClockPin, Limits, Verdict, Why};
use crate::save::Admit;
use crate::stats::Phase;

const ENTITIES_PER_TASK: usize = 256;
const RACY_INPUTS_PER_TASK: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub pos: [f32; 3],
    pub facing: f32,
}

#[derive(Clone, Debug)]
pub struct Stamped {
    pub conn: u32,
    pub nth: u32,
    pub received_ms: u32,
    pub input: Input,
}

#[derive(Clone, Debug)]
pub enum Input {
    Join(Hello),
    HostJoin(Hello),
    Claim(Claim),
    Teleport(Claim),
    Action(u32),
    Leave,
}

/// The order a tick applies each entity's inputs in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputOrder {
    /// As each connection sent them: the same inputs give the same world on any number of
    /// threads.
    Canonical,
    /// As worker threads happened to hand them over.
    Racy,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Body {
    pub movement: Movement,
    pub present: bool,
    pub clock: Option<ClockPin>,
    pub clock_spent_ms: u32,
    pub correction_seq: u32,
    pub corrected: Option<Correction>,
    pub teleported_at: Option<u32>,
    pub moved_at: u32,
    pub flags_changed_at: u32,
    pub refused: u32,
    pub stale: u32,
    pub rooted_at: Option<[f32; 2]>,
    pub placed: Option<Placement>,
}

impl Body {
    /// Stands the body still where it is, rooted if it was, for a new session to take: its clock
    /// and its last claim's time are the old client's, and a placement tells the new one the
    /// sequence its claims answer.
    fn hand_over(&mut self, tick: u32) -> Movement {
        let rooted = self.rooted_at.is_some();
        let still = Movement {
            flags: if rooted { flags::ROOT } else { 0 },
            pos: self.movement.pos,
            facing: self.movement.facing,
            ..Movement::default()
        };
        if still.flags != self.movement.flags {
            self.flags_changed_at = tick;
        }
        self.movement = still;
        self.moved_at = tick;
        self.clock = None;
        self.clock_spent_ms = 0;
        self.correction_seq = self.correction_seq.wrapping_add(1);
        self.placed = Some(Placement { tick, rooted });
        still
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correction {
    pub tick: u32,
    pub why: Why,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub tick: u32,
    pub rooted: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Act {
    Claim {
        id: u32,
        received_ms: u32,
        claim: Claim,
    },
    Teleport {
        id: u32,
        received_ms: u32,
        claim: Claim,
        may_teleport: bool,
    },
    Leave {
        id: u32,
    },
}

impl Act {
    pub fn id(&self) -> u32 {
        match *self {
            Self::Claim { id, .. } | Self::Teleport { id, .. } | Self::Leave { id } => id,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Stepped {
    pub claims: u32,
    pub refused: [u32; Why::ALL.len()],
    pub stale: u32,
    pub refusals: Vec<Refusal>,
}

/// A refused claim or teleport, and the last accepted movement it was judged against.
#[derive(Clone, Debug)]
pub struct Refusal {
    pub tick: u32,
    pub id: u32,
    pub name: String,
    pub why: Why,
    pub received_ms: u32,
    pub last: Movement,
    pub claim: Movement,
}

pub struct Admitted {
    pub conn: u32,
    pub id: u32,
    pub spawn: Movement,
}

pub struct Admission {
    pub tick: u32,
    pub admitted: Vec<Admitted>,
    pub taken_over: Vec<Admitted>,
    pub refused: Vec<u32>,
}

pub struct World {
    tick: u32,
    prev: Vec<Body>,
    next: Vec<Body>,
    names: Vec<String>,
    looks: Vec<Appearance>,
    hosts: Vec<bool>,
    id_of: HashMap<u32, u32>,
    spawns: Vec<Spawn>,
    limits: Limits,
    keep_refusals: bool,
}

impl World {
    pub fn new(spawns: Vec<Spawn>, limits: Limits) -> Self {
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
            hosts: Vec::new(),
            id_of: HashMap::new(),
            spawns,
            limits,
            keep_refusals: false,
        }
    }

    pub fn keep_refusals(&mut self) {
        self.keep_refusals = true;
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    pub fn spawns(&self) -> &[Spawn] {
        &self.spawns
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn stepped(&self) -> &[Body] {
        &self.next
    }

    pub fn before(&self) -> &[Body] {
        &self.prev
    }

    pub fn name(&self, id: u32) -> &str {
        &self.names[id as usize]
    }

    pub fn look(&self, id: u32) -> &Appearance {
        &self.looks[id as usize]
    }

    pub fn present(&self) -> usize {
        self.next.iter().filter(|b| b.present).count()
    }

    pub fn admit(
        &mut self,
        inputs: &[Stamped],
        mut decide: impl FnMut(&str) -> Admit,
    ) -> Admission {
        let (mut admitted, mut taken_over, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for s in inputs {
            let (Input::Join(hello) | Input::HostJoin(hello)) = &s.input else {
                continue;
            };
            if self.id_of.contains_key(&s.conn) {
                continue;
            }
            let id = self.prev.len() as u32;
            let host = matches!(s.input, Input::HostJoin(_));
            let spawn = match decide(&hello.name) {
                Admit::AtSpawn => self.spawns[id as usize % self.spawns.len()],
                Admit::Back(spawn) => spawn,
                Admit::Here => {
                    match self.take_over(s.conn, &hello.name, host) {
                        Some(taken) => taken_over.push(taken),
                        None => refused.push(s.conn),
                    }
                    continue;
                }
            };
            let body = Body {
                movement: Movement {
                    pos: spawn.pos,
                    facing: spawn.facing,
                    ..Movement::default()
                },
                present: true,
                moved_at: self.tick,
                flags_changed_at: self.tick,
                ..Body::default()
            };
            self.prev.push(body);
            self.next.push(body);
            self.names.push(hello.name.clone());
            self.looks.push(hello.appearance);
            self.hosts.push(host);
            self.id_of.insert(s.conn, id);
            admitted.push(Admitted {
                conn: s.conn,
                id,
                spawn: body.movement,
            });
        }
        Admission {
            tick: self.tick,
            admitted,
            taken_over,
            refused,
        }
    }

    fn take_over(&mut self, conn: u32, name: &str, host: bool) -> Option<Admitted> {
        let id = (0..self.prev.len())
            .rev()
            .find(|&i| self.prev[i].present && self.names[i] == name)?;
        if self.hosts[id] && !host {
            return None;
        }
        let id32 = id as u32;
        self.id_of.retain(|_, body| *body != id32);
        self.id_of.insert(conn, id32);
        self.hosts[id] = host;
        let spawn = self.prev[id].hand_over(self.tick);
        Some(Admitted {
            conn,
            id: id32,
            spawn,
        })
    }

    pub fn route(&self, inputs: &[Stamped], order: InputOrder) -> Vec<Act> {
        let acts: Vec<Act> = inputs
            .iter()
            .filter_map(|s| {
                let id = *self.id_of.get(&s.conn)?;
                match &s.input {
                    Input::Join(_) | Input::HostJoin(_) | Input::Action(_) => None,
                    Input::Claim(claim) => Some(Act::Claim {
                        id,
                        received_ms: s.received_ms,
                        claim: *claim,
                    }),
                    Input::Teleport(claim) => Some(Act::Teleport {
                        id,
                        received_ms: s.received_ms,
                        claim: *claim,
                        may_teleport: self.hosts[id as usize],
                    }),
                    Input::Leave => Some(Act::Leave { id }),
                }
            })
            .collect();
        let mut acts = match order {
            InputOrder::Canonical => acts,
            InputOrder::Racy => {
                let arrived = Mutex::new(Vec::with_capacity(acts.len()));
                acts.par_chunks(RACY_INPUTS_PER_TASK).for_each(|c| {
                    if let Ok(mut a) = arrived.lock() {
                        a.extend(c.iter().rev());
                    }
                });
                arrived.into_inner().unwrap_or_default()
            }
        };
        acts.sort_by_key(Act::id);
        acts
    }

    pub fn actions(&self, inputs: &[Stamped]) -> Vec<(u32, u32)> {
        let mut actions: Vec<(u32, u32)> = inputs
            .iter()
            .filter_map(|s| match s.input {
                Input::Action(number) => Some((*self.id_of.get(&s.conn)?, number)),
                _ => None,
            })
            .filter(|&(id, _)| self.next[id as usize].present)
            .collect();
        actions.sort_by_key(|a| a.0);
        actions
    }

    pub fn order(&mut self, orders: &[(u32, BodyOrder)]) {
        let tick = self.tick;
        for &(id, order) in orders {
            let Some(b) = self.next.get_mut(id as usize).filter(|b| b.present) else {
                continue;
            };
            let mut m = b.movement;
            if let Some(at) = order.place {
                (m.pos, m.facing) = (at.pos, at.facing);
            }
            let rooted = order.root.unwrap_or(b.rooted_at.is_some());
            let still = if rooted { flags::ROOT } else { 0 };
            if order.place.is_some() || rooted {
                m = Movement {
                    time: m.time,
                    flags: still,
                    pos: m.pos,
                    facing: m.facing,
                    ..Movement::default()
                };
            } else {
                m.flags &= !flags::ROOT;
            }
            if m.flags != b.movement.flags {
                b.flags_changed_at = tick;
            }
            b.rooted_at = rooted.then_some([m.pos[0], m.pos[1]]);
            b.movement = m;
            b.moved_at = tick;
            b.correction_seq = b.correction_seq.wrapping_add(1);
            b.placed = Some(Placement { tick, rooted });
        }
    }

    pub fn step(&mut self, acts: &[Act], phase: &Phase) -> Stepped {
        debug_assert!(acts.is_sorted_by_key(Act::id));
        let prev = &self.prev;
        let judge = Judge {
            limits: &self.limits,
            tick: self.tick,
            keep_refusals: self.keep_refusals,
        };
        let mut stepped = self
            .next
            .par_chunks_mut(ENTITIES_PER_TASK)
            .enumerate()
            .map(|(ci, chunk)| {
                phase.time(|| {
                    let lo = (ci * ENTITIES_PER_TASK) as u32;
                    let hi = lo + chunk.len() as u32;
                    chunk.copy_from_slice(&prev[lo as usize..hi as usize]);
                    let a = acts.partition_point(|a| a.id() < lo);
                    let b = acts.partition_point(|a| a.id() < hi);
                    let mut done = Stepped::default();
                    for act in &acts[a..b] {
                        let body = &mut chunk[(act.id() - lo) as usize];
                        judge.apply(body, act, &mut done);
                    }
                    done
                })
            })
            .reduce(Stepped::default, |mut x, y| {
                x.claims += y.claims;
                x.stale += y.stale;
                for (a, b) in x.refused.iter_mut().zip(y.refused) {
                    *a += b;
                }
                x.refusals.extend(y.refusals);
                x
            });
        for r in &mut stepped.refusals {
            r.name.clone_from(&self.names[r.id as usize]);
        }
        stepped
    }

    pub fn hash(&self, phase: &Phase) -> u64 {
        self.next
            .par_chunks(ENTITIES_PER_TASK)
            .enumerate()
            .map(|(ci, chunk)| {
                phase.time(|| {
                    chunk.iter().enumerate().fold(0u64, |h, (j, b)| {
                        h.wrapping_add(hash_body((ci * ENTITIES_PER_TASK + j) as u32, b))
                    })
                })
            })
            .reduce(|| 0, u64::wrapping_add)
    }

    pub fn finish(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
        self.tick += 1;
    }
}

struct Judge<'a> {
    limits: &'a Limits,
    tick: u32,
    keep_refusals: bool,
}

impl Judge<'_> {
    fn apply(&self, body: &mut Body, act: &Act, done: &mut Stepped) {
        if !body.present {
            return;
        }
        let tick = self.tick;
        let (received_ms, claim, verdict) = match *act {
            Act::Claim {
                received_ms, claim, ..
            } => (
                received_ms,
                claim,
                self.limits.judge(body, &claim, received_ms),
            ),
            Act::Teleport {
                received_ms,
                claim,
                may_teleport,
                ..
            } => {
                let verdict = self
                    .limits
                    .judge_teleport(body, &claim, received_ms, may_teleport);
                (received_ms, claim, verdict)
            }
            Act::Leave { .. } => {
                body.present = false;
                body.moved_at = tick;
                return;
            }
        };
        done.claims += 1;
        match verdict {
            Verdict::Accept => {
                let mut m = claim.movement;
                if body.rooted_at.is_some() {
                    m.flags |= flags::ROOT;
                }
                self.limits.pin_clock(body, m.time, received_ms);
                if m.flags != body.movement.flags {
                    body.flags_changed_at = tick;
                }
                body.movement = m;
                body.moved_at = tick;
                if matches!(act, Act::Teleport { .. }) {
                    body.teleported_at = Some(tick);
                }
            }
            Verdict::Stale => {
                body.stale += 1;
                done.stale += 1;
            }
            Verdict::Refuse(why) => {
                if self.keep_refusals {
                    done.refusals.push(Refusal {
                        tick,
                        id: act.id(),
                        name: String::new(),
                        why,
                        received_ms,
                        last: body.movement,
                        claim: claim.movement,
                    });
                }
                body.correction_seq = body.correction_seq.wrapping_add(1);
                body.corrected = Some(Correction { tick, why });
                body.refused += 1;
                done.refused[why as usize] += 1;
            }
        }
    }
}

fn hash_body(id: u32, b: &Body) -> u64 {
    let m = &b.movement;
    let pin = b.clock.unwrap_or_default();
    let words = [
        id,
        u32::from(b.present) | u32::from(b.rooted_at.is_some()) << 1,
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
        u32::from(b.clock.is_some()),
        pin.client_ms,
        pin.server_ms,
        b.clock_spent_ms,
        b.correction_seq,
        b.corrected.map_or(u32::MAX, |c| c.tick),
        b.moved_at,
        b.flags_changed_at,
        b.refused,
        b.stale,
    ];
    let fnv = |h: u64, w: u32| (h ^ u64::from(w)).wrapping_mul(0x0100_0000_01b3);
    let mut h = words.iter().fold(0xcbf2_9ce4_8422_2325, |h, &w| fnv(h, w));
    if let Some([x, y]) = b.rooted_at {
        h = fnv(fnv(h, x.to_bits()), y.to_bits());
    }
    if let Some(p) = b.placed {
        h = fnv(fnv(h, p.tick), u32::from(p.rooted));
    }
    h
}
