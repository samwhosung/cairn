use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Record, ServerMessage, VERSION,
    flags,
};
use server::{Config, Rules, Spawn};
use world::unit::CharacterLook;

use super::MEADOW;
use super::pair::{BOUND_ACROSS, HZ, copy_of};
use super::walker::Walker;
use crate::player::state::RUN_SPEED;

const LIE: f32 = 3.0;

struct Liar {
    stream: TcpStream,
    frames: Frames,
    id: u32,
    ack: u32,
    anchor_pos: [f32; 3],
    anchor_ms: u32,
    started: Instant,
    every: Duration,
    next: Duration,
    corrections: u32,
}

impl Liar {
    fn join(addr: SocketAddr, every: Duration) -> Self {
        let mut stream = TcpStream::connect(addr).expect("the liar connects");
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
        stream.write_all(&hello).expect("hello");
        stream.set_nonblocking(true).expect("non-blocking");
        let mut liar = Self {
            stream,
            frames: Frames::default(),
            id: u32::MAX,
            ack: 0,
            anchor_pos: [0.0; 3],
            anchor_ms: 0,
            started: Instant::now(),
            every,
            next: Duration::ZERO,
            corrections: 0,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while liar.id == u32::MAX {
            assert!(Instant::now() < deadline, "no welcome for the liar");
            liar.take();
            std::thread::sleep(Duration::from_millis(5));
        }
        liar
    }

    fn now_ms(&self) -> u32 {
        self.started.elapsed().as_millis() as u32
    }

    fn take(&mut self) {
        let mut buf = [0u8; 16 << 10];
        loop {
            match self.stream.read(&mut buf) {
                Ok(0) => panic!("the server hung up on the liar"),
                Ok(n) => self.frames.extend(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("the liar's connection: {e}"),
            }
        }
        let now = self.now_ms();
        while let Some(frame) = self.frames.next_frame().expect("frames") {
            match ServerMessage::read(frame).expect("a message") {
                ServerMessage::Welcome(w) => {
                    self.id = w.id;
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
        let now = self.started.elapsed();
        if now < self.next {
            return;
        }
        self.next = now + self.every;
        let t = self.now_ms();
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
        let _ = self.stream.write_all(&bytes);
    }
}

struct Seen {
    corrections: u32,
    worst_past_honest_reach: f32,
}

fn lie_beside(every: Duration, check: bool) -> Option<Seen> {
    let spawn = |dy: f32| Spawn {
        pos: [MEADOW[0], MEADOW[1] + dy, 59.86],
        facing: 0.0,
    };
    let server = server::start(Config {
        tick_threads: 1,
        io_threads: 1,
        spawns: vec![spawn(0.0), spawn(3.0)],
        rules: Rules {
            check,
            ..Rules::default()
        },
        ..Config::default()
    })
    .expect("a server");
    let mut honest = Walker::joined(server.addr(), "B", CharacterLook::naked(1, 0), HZ)?;
    let mut liar = Liar::join(server.addr(), every);
    let from = spawn(3.0).pos;
    let mut beyond = f32::MIN;
    let begun = Instant::now();
    for _ in 0..(3.0 * HZ) as usize {
        liar.claim_when_due();
        liar.take();
        honest.run(1);
        if let Some((seen, _)) = copy_of(&mut honest, liar.id) {
            let t = begun.elapsed().as_secs_f32();
            let honest_reach = RUN_SPEED * 1.1 * t + 0.5 + BOUND_ACROSS;
            let off = (seen[0] - from[0]).hypot(seen[1] - from[1]);
            beyond = beyond.max(off - honest_reach);
        }
    }
    Some(Seen {
        corrections: liar.corrections,
        worst_past_honest_reach: beyond,
    })
}

#[test]
fn a_client_claiming_three_times_its_speed_is_put_back_and_never_seen_to_lie() {
    let heartbeat = Duration::from_millis(500);
    let Some(checked) = lie_beside(heartbeat, true) else {
        return;
    };
    let unchecked = lie_beside(heartbeat, false).expect("the install");
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
