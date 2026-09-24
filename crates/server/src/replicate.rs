use protocol::{begin_batch, finish_frame, write_appear, write_correct, write_move, write_vanish};

use crate::grid::Grid;
use crate::net::Outbox;
use crate::world::{Body, World};

/// How far a player sees, and how often what it sees is refreshed.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Entities within this many yards come into view...
    pub radius: f32,
    /// ...and leave it only past this many more.
    pub grey: f32,
    /// Ticks between rechecks of who is in view, staggered across observers.
    pub aoi_every: u32,
    /// Movement refreshes by distance, nearest tier first. A change of movement flags goes out
    /// at once at any distance.
    pub tiers: [Tier; 3],
    /// A client with more than this many bytes queued gets no refreshes until it drains...
    pub shed_bytes: usize,
    /// ...and is dropped past this many.
    pub kick_bytes: usize,
}

/// Within `within` yards, at most one movement refresh per `every` ticks.
#[derive(Clone, Copy, Debug)]
pub struct Tier {
    pub within: f32,
    pub every: u32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            radius: 100.0,
            grey: 1.0,
            aoi_every: 5,
            tiers: [
                Tier {
                    within: 25.0,
                    every: 1,
                },
                Tier {
                    within: 50.0,
                    every: 4,
                },
                Tier {
                    within: f32::INFINITY,
                    every: 10,
                },
            ],
            shed_bytes: 256 << 10,
            kick_bytes: 16 << 20,
        }
    }
}

impl View {
    fn every(&self, d2: f32) -> u32 {
        self.tiers
            .iter()
            .find(|t| d2 <= t.within * t.within)
            .map_or(1, |t| t.every)
    }
}

/// An entity in view and the tick its movement last went out.
#[derive(Clone, Copy, Debug)]
struct Seen {
    slot: u32,
    sent: u32,
}

/// A player's view of the world, and the connection its batches go to.
pub struct Observer {
    pub slot: u32,
    seen: Vec<Seen>,
    fresh: bool,
    size_hint: usize,
    pub outbox: Option<Outbox>,
}

impl Observer {
    pub fn new(slot: u32, outbox: Option<Outbox>) -> Self {
        Self {
            slot,
            seen: Vec::new(),
            fresh: true,
            size_hint: 64,
            outbox,
        }
    }
}

/// What one tick's replication did, summed over observers.
#[derive(Clone, Copy, Debug, Default)]
pub struct Built {
    pub appeared: u32,
    pub vanished: u32,
    pub moves: u32,
    pub deferred: u32,
    pub corrections: u32,
    pub kicked: u32,
    pub bytes: u64,
}

impl Built {
    pub fn add(self, o: Self) -> Self {
        Self {
            appeared: self.appeared + o.appeared,
            vanished: self.vanished + o.vanished,
            moves: self.moves + o.moves,
            deferred: self.deferred + o.deferred,
            corrections: self.corrections + o.corrections,
            kicked: self.kicked + o.kicked,
            bytes: self.bytes + o.bytes,
        }
    }
}

/// Buffers one worker reuses across the observers it builds for.
#[derive(Default)]
pub struct Scratch {
    near: Vec<u32>,
    seen: Vec<Seen>,
}

/// Everything one observer's pass reads.
pub struct Scene<'a> {
    pub world: &'a World,
    pub grid: &'a Grid,
    pub view: &'a View,
}

/// Builds this tick's batch for `o` and hands it to its connection: a correction if its own
/// claim was refused, who came into and left view, and the movement it is owed.
pub fn build(o: &mut Observer, scene: &Scene<'_>, s: &mut Scratch) -> Built {
    let mut built = Built::default();
    let queued = o.outbox.as_ref().map_or(0, Outbox::queued);
    if queued > scene.view.kick_bytes {
        o.outbox = None;
        built.kicked = 1;
    }
    let world = scene.world;
    let tick = world.tick();
    let bodies = world.bodies();
    let me = bodies[o.slot as usize];
    let mut out = Vec::with_capacity(o.size_hint);
    let start = begin_batch(&mut out, tick);
    if me.corrected == tick {
        write_correct(&mut out, me.seq, &me.movement);
        built.corrections += 1;
    }
    let mut pass = Pass {
        me: &me,
        world,
        bodies,
        view: scene.view,
        tick,
        shedding: queued > scene.view.shed_bytes,
        out: &mut out,
        built: &mut built,
    };
    if o.fresh || (tick + o.slot).is_multiple_of(scene.view.aoi_every.max(1)) {
        s.near.clear();
        scene.grid.query(
            bodies,
            me.movement.pos,
            scene.view.radius + scene.view.grey,
            &mut s.near,
        );
        s.near.retain(|&n| n != o.slot);
        s.near.sort_unstable();
        pass.recheck(&o.seen, &s.near, &mut s.seen);
        std::mem::swap(&mut o.seen, &mut s.seen);
        o.fresh = false;
    } else {
        o.seen.retain_mut(|e| pass.keep(e));
    }
    finish_frame(&mut out, start);
    built.bytes = out.len() as u64;
    o.size_hint = out.len();
    if let Some(outbox) = &o.outbox {
        outbox.send(out);
    }
    built
}

struct Pass<'a> {
    me: &'a Body,
    world: &'a World,
    bodies: &'a [Body],
    view: &'a View,
    tick: u32,
    shedding: bool,
    out: &'a mut Vec<u8>,
    built: &'a mut Built,
}

impl Pass<'_> {
    fn dist2(&self, b: &Body) -> f32 {
        let (p, q) = (self.me.movement.pos, b.movement.pos);
        (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)
    }

    /// Merges who was in view with who is near now, both sorted by slot, into `next`.
    fn recheck(&mut self, seen: &[Seen], near: &[u32], next: &mut Vec<Seen>) {
        next.clear();
        let r2 = self.view.radius * self.view.radius;
        let (mut i, mut j) = (0, 0);
        loop {
            match (seen.get(i), near.get(j)) {
                (Some(&e), Some(&n)) if e.slot == n => {
                    let mut e = e;
                    self.refresh(&mut e);
                    next.push(e);
                    i += 1;
                    j += 1;
                }
                (Some(&e), n) if n.is_none_or(|&n| e.slot < n) => {
                    write_vanish(self.out, e.slot);
                    self.built.vanished += 1;
                    i += 1;
                }
                (_, Some(&n)) => {
                    if self.dist2(&self.bodies[n as usize]) <= r2 {
                        self.appear(n);
                        next.push(Seen {
                            slot: n,
                            sent: self.tick,
                        });
                    }
                    j += 1;
                }
                _ => break,
            }
        }
    }

    fn appear(&mut self, slot: u32) {
        let b = &self.bodies[slot as usize];
        let (name, look) = (self.world.name(slot), self.world.look(slot));
        write_appear(self.out, slot, name, look, &b.movement);
        self.built.appeared += 1;
    }

    /// Between rechecks: a dead entity leaves view; a living one gets what it is owed.
    fn keep(&mut self, e: &mut Seen) -> bool {
        if !self.bodies[e.slot as usize].alive {
            write_vanish(self.out, e.slot);
            self.built.vanished += 1;
            return false;
        }
        self.refresh(e);
        true
    }

    /// Sends `e`'s movement if it changed since last sent: at once for a change of flags,
    /// otherwise once its distance tier's period has passed and the client is keeping up.
    fn refresh(&mut self, e: &mut Seen) {
        let b = &self.bodies[e.slot as usize];
        if b.changed <= e.sent {
            return;
        }
        if b.turned <= e.sent {
            if self.tick - e.sent < self.view.every(self.dist2(b)) {
                return;
            }
            if self.shedding {
                self.built.deferred += 1;
                return;
            }
        }
        write_move(self.out, e.slot, &b.movement);
        e.sent = self.tick;
        self.built.moves += 1;
    }
}
