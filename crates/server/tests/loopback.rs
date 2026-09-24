use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Record, ServerMessage, VERSION,
    flags,
};
use server::{Config, Spawn, Window};

#[derive(Debug, PartialEq)]
enum Got {
    Appear(u32),
    Vanish(u32),
    Move(u32, [f32; 3]),
    Correct(u32),
}

struct Client {
    stream: TcpStream,
    frames: Frames,
    id: u32,
}

impl Client {
    fn join(addr: SocketAddr, name: &str) -> Self {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("a timeout");
        let mut hello = Vec::new();
        ClientMessage::Hello(Hello {
            version: VERSION,
            name: name.into(),
            appearance: Appearance::default(),
        })
        .write(&mut hello);
        stream.write_all(&hello).expect("hello");
        let mut client = Self {
            stream,
            frames: Frames::default(),
            id: 0,
        };
        let frame = client.frame();
        let Ok(ServerMessage::Welcome(w)) = ServerMessage::read(&frame) else {
            panic!("no welcome");
        };
        client.id = w.id;
        client
    }

    fn frame(&mut self) -> Vec<u8> {
        let mut buf = [0u8; 4096];
        loop {
            if let Some(f) = self.frames.next_frame().expect("well formed") {
                return f.to_vec();
            }
            let n = self.stream.read(&mut buf).expect("the server writes");
            assert!(n > 0, "the server closed the connection");
            self.frames.extend(&buf[..n]);
        }
    }

    fn batch(&mut self) -> Vec<Got> {
        let frame = self.frame();
        let Ok(ServerMessage::Batch(b)) = ServerMessage::read(&frame) else {
            panic!("not a batch");
        };
        b.map(|r| match r.expect("a valid record") {
            Record::Appear { id, .. } => Got::Appear(id),
            Record::Vanish { id } => Got::Vanish(id),
            Record::Move { id, movement } => Got::Move(id, movement.pos),
            Record::Correct { seq, .. } => Got::Correct(seq),
        })
        .collect()
    }

    fn records_until(&mut self, ticks: usize, want: impl Fn(&Got) -> bool) -> Vec<Got> {
        let mut seen = Vec::new();
        for _ in 0..ticks {
            let batch = self.batch();
            let done = batch.iter().any(&want);
            seen.extend(batch);
            if done {
                break;
            }
        }
        seen
    }

    fn claim(&mut self, ack: u32, time: u32, pos: [f32; 3]) {
        let mut bytes = Vec::new();
        ClientMessage::Claim(Claim {
            ack,
            movement: Movement {
                time,
                flags: flags::FORWARD,
                pos,
                ..Movement::default()
            },
        })
        .write(&mut bytes);
        self.stream.write_all(&bytes).expect("a claim");
    }
}

#[test]
fn two_clients_over_loopback_see_each_other_move_but_never_a_refused_claim() {
    let spawns = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]
        .map(|pos| Spawn { pos, facing: 0.0 })
        .to_vec();
    let running = server::start(Config {
        spawns,
        tick_threads: 2,
        ..Config::default()
    })
    .expect("a server");
    let mut a = Client::join(running.addr(), "Ada");
    let mut b = Client::join(running.addr(), "Bo");
    assert_ne!(a.id, b.id);
    let appeared = a.records_until(40, |g| *g == Got::Appear(b.id));
    assert!(appeared.contains(&Got::Appear(b.id)), "{appeared:?}");

    b.claim(0, 1000, [10.0, 0.0, 0.0]);
    let seen = a.records_until(40, |g| matches!(g, Got::Move(id, _) if *id == b.id));
    assert!(
        seen.contains(&Got::Move(b.id, [10.0, 0.0, 0.0])),
        "{seen:?}"
    );

    b.claim(0, 1500, [90.0, 0.0, 0.0]);
    let own = b.records_until(40, |g| matches!(g, Got::Correct(_)));
    assert!(own.contains(&Got::Correct(1)), "{own:?}");
    b.claim(1, 1600, [10.5, 0.0, 0.0]);
    let seen = a.records_until(40, |g| matches!(g, Got::Move(id, _) if *id == b.id));
    assert!(
        seen.contains(&Got::Move(b.id, [10.5, 0.0, 0.0])),
        "{seen:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|g| matches!(g, Got::Move(_, p) if p[0] > 50.0)),
        "the refused claim was relayed: {seen:?}"
    );

    drop(b);
    let seen = a.records_until(40, |g| matches!(g, Got::Vanish(_)));
    assert!(seen.contains(&Got::Vanish(1)), "{seen:?}");
    let summary = running.stop().expect("a clean stop");
    assert_eq!(
        summary.refused,
        [0, 0, 1, 0, 0, 0],
        "one refusal, for speed"
    );
}

#[test]
fn a_crowd_that_leaves_before_the_window_closes_stops_the_server() {
    let running = server::start(Config {
        tick_threads: 1,
        window: Some(Window {
            players: 1,
            settle: 10_000,
            measure: 10_000,
        }),
        ..Config::default()
    })
    .expect("a server");
    drop(Client::join(running.addr(), "Ada"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(running.wait().map(|s| s.ticks)));
    let measured = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the server stops")
        .expect("a clean stop");
    assert_eq!(measured, 0, "the window never opened");
}
