//! A client that claims three times its speed beside a real one: the server puts the liar back
//! each time, and the real client never sees it anywhere an honest runner could not have been.

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

const RUN: f32 = 7.0;
const LIE: f32 = 3.0;

/// Runs north claiming `LIE` times the ground it covers, every `every` of its own clock, and
/// takes each correction it is sent.
struct Liar {
    stream: TcpStream,
    frames: Frames,
    id: u32,
    ack: u32,
    /// Where the server last put it, and when on its clock.
    anchor: ([f32; 3], u32),
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
            anchor: ([0.0; 3], 0),
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
                    self.anchor = (w.spawn.pos, now);
                }
                ServerMessage::Batch(batch) => {
                    for record in batch {
                        if let Ok(Record::Correct { seq, movement }) = record {
                            self.ack = seq;
                            self.anchor = (movement.pos, now);
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
        let ([x, y, z], since) = self.anchor;
        let run = LIE * RUN * t.saturating_sub(since) as f32 / 1000.0;
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
    /// How much further from where it started the real client saw the liar than an honest
    /// runner could have been, at the worst.
    beyond_honest: f32,
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
            let honest_reach = RUN * 1.1 * t + 0.5 + BOUND_ACROSS;
            let off = (seen[0] - from[0]).hypot(seen[1] - from[1]);
            beyond = beyond.max(off - honest_reach);
        }
    }
    Some(Seen {
        corrections: liar.corrections,
        beyond_honest: beyond,
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
            s.corrections, s.beyond_honest
        );
    }
    assert!(checked.corrections > 0 && checked.beyond_honest <= 0.0);
    assert!(unchecked.beyond_honest > 0.0, "the control saw no lie");
}
