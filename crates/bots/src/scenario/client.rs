use std::collections::{HashMap, VecDeque};

use protocol::{
    Claim, ClientMessage, LEN_BYTES, Movement, Pos, Record, ServerMessage, Welcome, Wrapped,
};
use server::{Input, Link, Spawn, Stamped};

use super::spec::Script;
use crate::ground::Ground;
use crate::lie::Lie;
use crate::mover::{Claims, Frame, Mover};
use crate::region::{Place, XorShift64Star};
use crate::track::{Pace, Track, Walk, plan};

const SEEN_EVERY_MS: u32 = 500;
const START_AFTER_WELCOME_MS: u32 = 100;

#[derive(Clone, Debug)]
pub struct Brief {
    pub group: usize,
    pub spawn: Spawn,
    pub script: Script,
    pub pace: Pace,
    pub lies: Vec<Lie>,
    pub frame_hz: f32,
    pub claims: Claims,
    pub clock_at_start_ms: u32,
    pub seed: u64,
    pub route_until_ms: u32,
}

pub struct Outgoing {
    pub arrives_ms: u32,
    pub bytes: Vec<u8>,
    pub made_in: Option<Frame>,
}

pub struct Delivered {
    pub claim: Claim,
    pub made_in: Frame,
}

/// A liar's position the server accepted, and how far it stood from the liar's body.
#[derive(Clone, Copy, Debug)]
pub struct Accepted {
    pub pos: Pos,
    pub off_body_yd: f32,
}

pub struct World<'a> {
    pub place: &'a Place,
    pub ground: &'a Ground,
    pub liar_of: &'a [Option<usize>],
    pub accepted: &'a [Vec<Accepted>],
}

/// Where a client saw one liar, against what the server accepted from it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Seen {
    pub positions: u64,
    pub worst_yd: f32,
    pub misread: u64,
    pub unaccepted: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub corrections: u64,
    pub lies: u64,
    pub first_lie_ms: Option<u32>,
    pub decode_errors: u64,
    pub welcomed_as: Option<u32>,
    pub seen: Vec<Seen>,
}

struct Body {
    track: Track,
    mover: Mover,
}

struct Net {
    rng: XorShift64Star,
    delay_ms: u32,
    jitter_ms: u32,
    out_last: u32,
    in_last: u32,
}

impl Net {
    fn lag(&mut self) -> u32 {
        let jitter = self.rng.next_u64() % (u64::from(self.jitter_ms) + 1);
        self.delay_ms + jitter as u32
    }

    fn out(&mut self, sent_ms: u32) -> u32 {
        self.out_last = (sent_ms + self.lag()).max(self.out_last);
        self.out_last
    }

    fn back(&mut self, sent_ms: u32) -> u32 {
        self.in_last = (sent_ms + self.lag()).max(self.in_last);
        self.in_last
    }
}

/// A bot as a client of a server in the same process, stepped on the server's clock.
pub struct Client {
    pub conn: u32,
    pub brief: Brief,
    link: Link,
    net: Net,
    frame_n: u64,
    phase_ms: u32,
    inbound: VecDeque<(u32, Vec<u8>)>,
    outbound: VecDeque<Outgoing>,
    nth: u32,
    body: Option<Body>,
    seen_tick: u32,
    next_seen_ms: u32,
    watching: HashMap<u16, usize>,
    here: [f32; 3],
    claims: Vec<Movement>,
    pub tally: Tally,
}

impl Client {
    /// A client whose hello arrives after the bare delay, so every bot joins at the same tick.
    pub fn new(
        conn: u32,
        brief: Brief,
        link: Link,
        lag: (u32, u32),
        hello: Vec<u8>,
        liars: usize,
    ) -> Self {
        let mut rng = XorShift64Star::new(brief.seed ^ 0x6e65_7477_6f72_6b00);
        let period = 1000.0 / brief.frame_hz;
        let phase_ms = rng.range(0.0, period) as u32;
        let here = brief.spawn.pos;
        let (delay_ms, jitter_ms) = lag;
        Self {
            conn,
            brief,
            link,
            net: Net {
                rng,
                delay_ms,
                jitter_ms,
                out_last: delay_ms,
                in_last: 0,
            },
            frame_n: 0,
            phase_ms,
            inbound: VecDeque::new(),
            outbound: VecDeque::from([Outgoing {
                arrives_ms: delay_ms,
                bytes: hello,
                made_in: None,
            }]),
            nth: 0,
            body: None,
            seen_tick: 0,
            next_seen_ms: 0,
            watching: HashMap::new(),
            here,
            claims: Vec::new(),
            tally: Tally {
                seen: vec![Seen::default(); liars],
                ..Tally::default()
            },
        }
    }

    fn frame_at(&self, n: u64) -> u32 {
        self.phase_ms + (n as f64 * 1000.0 / f64::from(self.brief.frame_hz)) as u32
    }

    fn clock(&self, t: u32) -> u32 {
        self.brief.clock_at_start_ms + t
    }

    /// Runs every frame due by `until_ms`, each after reading what arrived before it.
    pub fn advance(&mut self, until_ms: u32, world: &World<'_>) {
        loop {
            let t = self.frame_at(self.frame_n);
            if t > until_ms {
                break;
            }
            while let Some((arrived, _)) = self.inbound.front()
                && *arrived <= t
            {
                let Some((_, frame)) = self.inbound.pop_front() else {
                    break;
                };
                self.link.taken_in(frame.len());
                self.take(&frame, t, world);
            }
            self.frame(t, world);
            self.frame_n += 1;
        }
    }

    /// Hands the server what has reached it by `now`: joins and claims as inputs, and reports of
    /// the latest tick seen as how far behind the client is before tick `next_tick`.
    pub fn deliver(
        &mut self,
        now: u32,
        next_tick: u32,
        inputs: &mut Vec<Stamped>,
        delivered: &mut Vec<(u32, Delivered)>,
    ) {
        while self.outbound.front().is_some_and(|o| o.arrives_ms <= now) {
            let Some(o) = self.outbound.pop_front() else {
                break;
            };
            let input = match ClientMessage::read(&o.bytes[LEN_BYTES..]) {
                Ok(ClientMessage::Seen(tick)) => {
                    self.link.behind_by(next_tick.saturating_sub(tick));
                    continue;
                }
                Ok(ClientMessage::Hello(h)) => Input::Join(h),
                Ok(ClientMessage::Claim(claim)) => {
                    if let Some(made_in) = o.made_in {
                        delivered.push((self.conn, Delivered { claim, made_in }));
                    }
                    Input::Claim(claim)
                }
                Ok(ClientMessage::Teleport(_)) | Err(_) => {
                    self.tally.decode_errors += 1;
                    continue;
                }
            };
            inputs.push(Stamped {
                conn: self.conn,
                nth: self.nth,
                received_ms: o.arrives_ms,
                input,
            });
            self.nth += 1;
        }
    }

    /// Takes in what the server sent it at `now`, to arrive after the network's lag.
    pub fn receive(&mut self, now: u32) {
        while let Some(frame) = self.link.next_frame() {
            let arrives = self.net.back(now);
            self.inbound.push_back((arrives, frame));
        }
    }

    fn take(&mut self, frame: &[u8], t: u32, world: &World<'_>) {
        let batch = match ServerMessage::read(&frame[LEN_BYTES..]) {
            Ok(ServerMessage::Welcome(w)) => {
                self.welcome(&w, t, world);
                return;
            }
            Ok(ServerMessage::Batch(batch)) => batch,
            Err(_) => {
                self.tally.decode_errors += 1;
                return;
            }
        };
        self.seen_tick = batch.tick;
        let watch = !world.accepted.is_empty() && self.brief.lies.is_empty();
        for record in batch {
            match record {
                Ok(Record::Correct { seq, .. }) => {
                    if let Some(b) = &mut self.body {
                        b.mover.correct(seq);
                    }
                    self.tally.corrections += 1;
                }
                Ok(_) if !watch => break,
                Ok(Record::Appear {
                    slot, id, state, ..
                }) => match world.liar_of.get(id as usize).copied().flatten() {
                    Some(liar) => {
                        self.watching.insert(slot, liar);
                        self.saw(liar, state.pos, world);
                    }
                    None => {
                        self.watching.remove(&slot);
                    }
                },
                Ok(Record::Move { slot, pos, .. }) => self.saw_slot(slot, pos, world),
                Ok(Record::State { slot, state }) => self.saw_slot(slot, state.pos, world),
                Ok(Record::Vanish { slot }) => {
                    self.watching.remove(&slot);
                }
                Ok(Record::Turn { .. }) => {}
                Err(_) => {
                    self.tally.decode_errors += 1;
                    break;
                }
            }
        }
    }

    fn saw_slot(&mut self, slot: u16, pos: Wrapped, world: &World<'_>) {
        if let Some(&liar) = self.watching.get(&slot) {
            self.saw(liar, pos, world);
        }
    }

    fn saw(&mut self, liar: usize, bits: Wrapped, world: &World<'_>) {
        let read = bits.around(self.here);
        let seen = &mut self.tally.seen[liar];
        let accepted = &world.accepted[liar];
        if let Some(a) = accepted.iter().rev().find(|a| a.pos == read) {
            seen.positions += 1;
            seen.worst_yd = seen.worst_yd.max(a.off_body_yd);
        } else if accepted.iter().any(|a| a.pos.wrapped() == bits) {
            seen.misread += 1;
        } else {
            seen.unaccepted += 1;
        }
    }

    fn welcome(&mut self, w: &Welcome, t: u32, world: &World<'_>) {
        let spawn = Spawn {
            pos: w.spawn.pos,
            facing: w.spawn.facing,
        };
        let start = self.clock(t) + START_AFTER_WELCOME_MS;
        let b = &self.brief;
        let until = b.route_until_ms.max(start);
        let track = match b.script {
            Script::Wander => {
                let walk = Walk {
                    start_ms: start,
                    until_ms: until,
                    seed: b.seed,
                    run_speed: b.pace.speed,
                    long_runs: false,
                };
                plan(world.place, world.ground, &spawn, &walk)
            }
            Script::Line => Track::line(&spawn, &b.pace, start, until),
            Script::Stand => {
                let stand = Pace {
                    stop_yd: Some(0.0),
                    ..b.pace
                };
                Track::line(&spawn, &stand, start, until)
            }
        };
        let mover = Mover::new(spawn.pos, spawn.facing, b.lies.clone(), b.claims);
        self.body = Some(Body { track, mover });
        self.here = spawn.pos;
        self.seen_tick = w.tick;
        self.next_seen_ms = t + SEEN_EVERY_MS;
        self.tally.welcomed_as = Some(w.id);
    }

    fn frame(&mut self, t: u32, world: &World<'_>) {
        let clock = self.clock(t);
        let Some(body) = &mut self.body else {
            return;
        };
        self.claims.clear();
        let f = body
            .mover
            .frame(clock, &body.track, world.ground, &mut self.claims);
        self.here = f.truth.pos;
        for &movement in &self.claims {
            let mut frame = Vec::new();
            ClientMessage::Claim(Claim {
                ack: body.mover.ack,
                movement,
            })
            .write(&mut frame);
            self.outbound.push_back(Outgoing {
                arrives_ms: self.net.out(t),
                bytes: frame,
                made_in: Some(f),
            });
        }
        if f.lied {
            self.tally.lies += f.claims as u64;
            self.tally.first_lie_ms.get_or_insert(t);
        }
        if t >= self.next_seen_ms {
            self.next_seen_ms = t + SEEN_EVERY_MS;
            let mut frame = Vec::new();
            ClientMessage::Seen(self.seen_tick).write(&mut frame);
            self.outbound.push_back(Outgoing {
                arrives_ms: self.net.out(t),
                bytes: frame,
                made_in: None,
            });
        }
    }
}
