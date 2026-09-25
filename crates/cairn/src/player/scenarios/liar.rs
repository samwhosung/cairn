use std::sync::mpsc::TryRecvError;

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Record, ServerMessage, VERSION,
    flags,
};
use server::{Config, InProcess, Limits, Spawn, Standing};
use world::unit::CharacterLook;

use super::MEADOW;
use super::clock::{self, SharedClock};
use super::pair::{BOUND_ACROSS, HZ, copy_of};
use super::walker::Walker;
use crate::player::state::RUN_SPEED;

const LIE: f32 = 3.0;

struct Liar {
    server: InProcess,
    clock: SharedClock,
    frames: Frames,
    id: Option<u32>,
    ack: u32,
    anchor_pos: [f32; 3],
    anchor_ms: u32,
    next_ms: u32,
    corrections: u32,
}

impl Liar {
    fn connect(clock: &SharedClock) -> Self {
        let server = clock.borrow_mut().connect(Standing::Guest);
        let mut hello = Vec::new();
        ClientMessage::Hello(Hello {
            version: VERSION,
            name: "Liar".into(),
            appearance: Appearance {
                race: 2,
                ..Appearance::default()
            },
        })
        .write(&mut hello);
        server.send(hello).expect("hello");
        Self {
            server,
            clock: clock.clone(),
            frames: Frames::default(),
            id: None,
            ack: 0,
            anchor_pos: [0.0; 3],
            anchor_ms: 0,
            next_ms: 0,
            corrections: 0,
        }
    }

    fn now_ms(&self) -> u32 {
        self.clock.borrow().ms()
    }

    fn take(&mut self) {
        loop {
            match self.server.try_recv() {
                Ok((bytes, _)) => self.frames.extend(&bytes),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => panic!("the server hung up on the liar"),
            }
        }
        let now = self.now_ms();
        while let Some(frame) = self.frames.next_frame().expect("frames") {
            match ServerMessage::read(frame).expect("a message") {
                ServerMessage::Welcome(w) => {
                    self.id = Some(w.id);
                    (self.anchor_pos, self.anchor_ms) = (w.spawn.pos, now);
                }
                ServerMessage::Batch(batch) => {
                    for record in batch {
                        if let Ok(Record::Correct { seq, movement, .. }) = record {
                            self.ack = seq;
                            (self.anchor_pos, self.anchor_ms) = (movement.pos, now);
                            self.corrections += 1;
                        }
                    }
                }
            }
        }
    }

    fn claim_when_due(&mut self) {
        let t = self.now_ms();
        if t < self.next_ms {
            return;
        }
        self.next_ms = t + protocol::HEARTBEAT_MS;
        let [x, y, z] = self.anchor_pos;
        let run = LIE * RUN_SPEED * t.saturating_sub(self.anchor_ms) as f32 / 1000.0;
        let mut bytes = Vec::new();
        ClientMessage::Claim(Claim {
            ack: self.ack,
            movement: Movement {
                time: t,
                flags: flags::FORWARD,
                pos: [x + run, y, z],
                ..Movement::default()
            },
        })
        .write(&mut bytes);
        let _ = self.server.send(bytes);
    }
}

struct Seen {
    corrections: u32,
    worst_past_honest_reach: f32,
}

fn lie_beside(check: bool) -> Option<Seen> {
    let spawn = |dy: f32| Spawn {
        pos: [MEADOW[0], MEADOW[1] + dy, 59.86],
        facing: 0.0,
    };
    let cfg = Config {
        tick_threads: 1,
        spawns: vec![spawn(0.0), spawn(3.0)],
        limits: Limits {
            check,
            ..Limits::default()
        },
        ..Config::default()
    };
    let clock = clock::serve(&cfg, clock::step_at(HZ));
    let mut honest = Walker::joined(&clock, "B", CharacterLook::naked(1, 0))?;
    let mut liar = Liar::connect(&clock);
    for _ in 0..(5.0 * HZ) as usize {
        if liar.id.is_some() {
            break;
        }
        liar.take();
        honest.run(1);
    }
    let id = liar.id.expect("a welcome for the liar");
    let from = spawn(3.0).pos;
    let mut beyond = f32::MIN;
    let begun = liar.now_ms();
    for _ in 0..(3.0 * HZ) as usize {
        liar.claim_when_due();
        liar.take();
        honest.run(1);
        if let Some((seen, _)) = copy_of(&mut honest, id) {
            let t = (liar.now_ms() - begun) as f32 / 1000.0;
            let honest_reach = RUN_SPEED * 1.1 * t + 0.5 + BOUND_ACROSS;
            let off = (seen[0] - from[0]).hypot(seen[1] - from[1]);
            beyond = beyond.max(off - honest_reach);
        }
    }
    drop(honest);
    clock.borrow_mut().stop();
    Some(Seen {
        corrections: liar.corrections,
        worst_past_honest_reach: beyond,
    })
}

#[test]
fn a_client_claiming_three_times_its_speed_is_put_back_and_never_seen_to_lie() {
    let Some(checked) = lie_beside(true) else {
        return;
    };
    let unchecked = lie_beside(false).expect("the install");
    for (what, s) in [("checked", &checked), ("unchecked", &unchecked)] {
        eprintln!(
            "a liar {what}: {} corrections, seen {:+.2} yd past an honest runner's reach",
            s.corrections, s.worst_past_honest_reach
        );
    }
    assert!(checked.corrections > 0 && checked.worst_past_honest_reach <= 0.0);
    assert!(
        unchecked.worst_past_honest_reach > 0.0,
        "the control saw no lie"
    );
}
