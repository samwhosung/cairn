use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use protocol::{
    Appearance, Claim, ClientMessage, Frames, Hello, Movement, Pos, Record, ServerMessage, VERSION,
    Why, flags,
};
use server::{
    Config, InProcess, InputOrder, Replay, Replicate, Running, Spawn, Summary, View, Window,
};

#[derive(Debug, PartialEq)]
enum Got {
    Appear(u32),
    Vanish(u32),
    Move(u32, [f32; 3]),
    Correct(u32, Why),
}

struct Client {
    stream: TcpStream,
    frames: Frames,
    id: u32,
    welcomed_at: u32,
    at: [f32; 3],
    slots: HashMap<u16, u32>,
}

impl Client {
    fn join(addr: Option<SocketAddr>, name: &str) -> Self {
        let mut stream = TcpStream::connect(addr.expect("a server that listens")).expect("connect");
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
            welcomed_at: 0,
            at: [0.0; 3],
            slots: HashMap::new(),
        };
        let frame = client.frame();
        let Ok(ServerMessage::Welcome(w)) = ServerMessage::read(&frame) else {
            panic!("no welcome");
        };
        client.id = w.id;
        client.welcomed_at = w.tick;
        client.at = w.spawn.pos;
        client
    }

    fn seen(&mut self, tick: u32) -> std::io::Result<()> {
        let mut bytes = Vec::new();
        ClientMessage::Seen(tick).write(&mut bytes);
        self.stream.write_all(&bytes)
    }

    fn cut_off(&mut self) -> bool {
        let refused = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(10));
            self.seen(0).is_err()
        });
        let mut buf = [0u8; 4096];
        let ran_out = loop {
            match self.stream.read(&mut buf) {
                Ok(0) => break true,
                Ok(_) => {}
                Err(e) => break !matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
            }
        };
        refused && ran_out
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
        let records: Vec<Record<'_>> = b.map(|r| r.expect("a valid record")).collect();
        let mut got = Vec::new();
        for r in records {
            let (slot, pos) = match r {
                Record::Appear { slot, id, .. } => {
                    self.slots.insert(slot, id);
                    got.push(Got::Appear(id));
                    continue;
                }
                Record::Vanish { slot } => {
                    got.push(Got::Vanish(self.slots.remove(&slot).expect("held")));
                    continue;
                }
                Record::Correct { seq, why, .. } => {
                    got.push(Got::Correct(seq, why));
                    continue;
                }
                Record::Turn { .. } => continue,
                Record::Move { slot, pos, .. } => (slot, pos),
                Record::State { slot, state } => (slot, state.pos),
            };
            got.push(Got::Move(self.slots[&slot], pos.around(self.at).yards()));
        }
        got
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
    let b_id = b.id;
    let relayed = |yd| Got::Move(b_id, Pos::of(yd).yards());
    assert!(seen.contains(&relayed([10.0, 0.0, 0.0])), "{seen:?}");

    b.claim(0, 1500, [90.0, 0.0, 0.0]);
    let own = b.records_until(40, |g| matches!(g, Got::Correct(..)));
    assert!(own.contains(&Got::Correct(1, Why::Speed)), "{own:?}");
    b.claim(1, 1600, [10.5, 0.0, 0.0]);
    let seen = a.records_until(40, |g| matches!(g, Got::Move(id, _) if *id == b.id));
    assert!(seen.contains(&relayed([10.5, 0.0, 0.0])), "{seen:?}");
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
        [0, 0, 1, 0, 0, 0, 0],
        "one refusal, for speed"
    );
}

#[test]
fn a_client_that_stops_reading_is_dropped_and_the_others_see_it_vanish() {
    let running = server::start(Config {
        tick_threads: 1,
        tick_ms: 10,
        view: View {
            kick_ticks: 5,
            ..View::default()
        },
        ..Config::default()
    })
    .expect("a server");
    let mut ada = Client::join(running.addr(), "Ada");
    let mut bo = Client::join(running.addr(), "Bo");
    ada.records_until(40, |g| *g == Got::Appear(bo.id));
    let stalled = bo.welcomed_at;
    let mut seen = Vec::new();
    for _ in 0..100 {
        let _ = bo.seen(stalled);
        let batch = ada.batch();
        let gone = batch.contains(&Got::Vanish(bo.id));
        seen.extend(batch);
        if gone {
            break;
        }
    }
    assert!(
        seen.contains(&Got::Vanish(bo.id)),
        "Bo stopped reading at tick {stalled} and Ada never saw it vanish: {seen:?}"
    );
    assert!(bo.cut_off(), "the server still holds Bo's connection");
    let summary = running.stop().expect("a clean stop");
    assert_eq!(summary.kicked, 1);
    assert!(
        ada.cut_off(),
        "the server stopped and left Ada's connection open"
    );
}

#[test]
fn a_crowd_that_leaves_before_the_window_closes_stops_the_server() {
    let running = server::start(Config {
        tick_threads: 1,
        window: Some(Window {
            players: 1,
            arrival: 10_000,
            settle: 10_000,
            measure: 10_000,
            grace: 10_000,
        }),
        ..Config::default()
    })
    .expect("a server");
    drop(Client::join(running.addr(), "Ada"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        tx.send(
            running
                .wait()
                .map(|s| (s.ticks, s.players_arrived, s.players_wanted)),
        )
    });
    let ended = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the server stops")
        .expect("a clean stop");
    assert_eq!(
        ended,
        (0, 1, 1),
        "the window never opened, its crowd in full"
    );
}

const WINDOW_TICK_MS: u16 = 20;

fn window_with<T>(window: Window, come: impl FnOnce(Option<SocketAddr>) -> T) -> (Summary, T) {
    let running = server::start(Config {
        tick_threads: 1,
        tick_ms: WINDOW_TICK_MS,
        window: Some(window),
        ..Config::default()
    })
    .expect("a server");
    let started = Instant::now();
    let clients = come(running.addr());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || tx.send(running.wait().map(Box::new)));
    let ticks = window.arrival + window.settle + window.measure + window.grace;
    let bound = Duration::from_millis(u64::from(ticks) * u64::from(WINDOW_TICK_MS)) * 2
        + Duration::from_secs(1);
    let summary = rx
        .recv_timeout(bound.saturating_sub(started.elapsed()))
        .unwrap_or_else(|_| {
            panic!("the window did not end within {bound:?}: twice its {ticks} ticks, and a second")
        })
        .expect("a clean stop");
    (*summary, clients)
}

#[test]
fn a_window_ends_on_time_though_a_client_neither_reads_nor_leaves() {
    let window = Window {
        players: 1,
        arrival: 20,
        settle: 5,
        measure: 10,
        grace: 20,
    };
    let (summary, mut stays) = window_with(window, |addr| Client::join(addr, "Ada"));
    assert_eq!(
        (summary.ticks, summary.ticks_after, summary.stayed),
        (10, 20, 1),
        "ticks measured, ticks after, players dropped"
    );
    assert!(
        stays.cut_off(),
        "the server returned and left the connection open"
    );
}

#[test]
fn a_window_ends_on_time_though_its_crowd_never_fully_arrives() {
    let window = Window {
        players: 5,
        arrival: 20,
        settle: 5,
        measure: 10,
        grace: 20,
    };
    let (left, ()) = window_with(window, |addr| drop(Client::join(addr, "Ada")));
    assert_eq!(
        (
            left.ticks,
            left.players_arrived,
            left.players_wanted,
            left.stayed
        ),
        (0, 0, 5, 0),
        "Ada came and left, and the window stopped waiting with nobody in"
    );
    let (stayed, _ada) = window_with(window, |addr| Client::join(addr, "Ada"));
    assert_eq!(
        (
            stayed.ticks,
            stayed.players_arrived,
            stayed.players_wanted,
            stayed.stayed
        ),
        (10, 1, 5, 1),
        "Ada came and stayed, alone, until the grace ran out"
    );
    assert!(stayed.row("").contains("| 1 of 5 |"), "{}", stayed.row(""));
}

#[test]
fn a_recorded_run_replays_with_every_batch_and_dumps_the_first_clients_frames() {
    let dir = std::env::temp_dir().join(format!("server-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let (log, dump) = (dir.join("inputs.log"), dir.join("frames.bin"));
    let spawns = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]
        .map(|pos| Spawn { pos, facing: 0.0 })
        .to_vec();
    let running = server::start(Config {
        spawns,
        tick_threads: 2,
        record: Some(log.clone()),
        ..Config::default()
    })
    .expect("a server");
    let mut a = Client::join(running.addr(), "Ada");
    let mut b = Client::join(running.addr(), "Bo");
    a.records_until(40, |g| *g == Got::Appear(b.id));
    b.claim(0, 1000, [10.0, 0.0, 0.0]);
    let seen = a.records_until(40, |g| matches!(g, Got::Move(..)));
    assert!(seen.iter().any(|g| matches!(g, Got::Move(..))), "{seen:?}");
    running.stop().expect("a clean stop");

    let how = Replay {
        threads: 2,
        order: InputOrder::Canonical,
        keep_refusals: false,
        replicate: Replicate::Dumping(&dump),
    };
    let r = server::replay(&log, &how).expect("a replay");
    assert_eq!(r.first_mismatch, None);
    assert!(r.summary.movements_per_client > 0.0);
    let mut frames = Frames::default();
    frames.extend(&std::fs::read(&dump).expect("a dump"));
    let first = frames
        .next_frame()
        .expect("framed")
        .expect("a welcome")
        .to_vec();
    assert!(matches!(
        ServerMessage::read(&first),
        Ok(ServerMessage::Welcome(_))
    ));
    let mut ticks = Vec::new();
    while let Some(frame) = frames.next_frame().expect("framed") {
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(frame) else {
            panic!("not a batch");
        };
        ticks.push(batch.tick);
    }
    assert!(!ticks.is_empty() && ticks.len() <= r.ticks as usize);
    assert!(
        ticks.windows(2).all(|w| w[1] == w[0] + 1),
        "one batch a tick"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn hello(name: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: name.into(),
        appearance: Appearance::default(),
    })
    .write(&mut bytes);
    bytes
}

fn teleport_to(time: u32, pos: [f32; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    ClientMessage::Teleport(Claim {
        ack: 0,
        movement: Movement {
            time,
            pos,
            ..Movement::default()
        },
    })
    .write(&mut bytes);
    bytes
}

/// Every frame the host's connection has been handed within `for_`, until one `until` wants.
fn host_frames(
    host: &InProcess,
    for_: Duration,
    until: impl Fn(&ServerMessage<'_>) -> bool,
) -> Vec<Vec<u8>> {
    let (mut frames, mut got) = (Frames::default(), Vec::new());
    let deadline = Instant::now() + for_;
    while Instant::now() < deadline {
        let Ok((bytes, _)) = host.try_recv() else {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        };
        frames.extend(&bytes);
        while let Some(frame) = frames.next_frame().expect("framed") {
            let done = until(&ServerMessage::read(frame).expect("a message"));
            got.push(frame.to_vec());
            if done {
                return got;
            }
        }
    }
    got
}

fn host_joins(running: &Running) -> (InProcess, u32) {
    let host = running.host_joins();
    host.send(hello("Host"))
        .expect("the server takes the hello");
    let welcomed = host_frames(&host, Duration::from_secs(5), |m| {
        matches!(m, ServerMessage::Welcome(_))
    });
    let Some(Ok(ServerMessage::Welcome(w))) = welcomed.last().map(|f| ServerMessage::read(f))
    else {
        panic!("no welcome for the host");
    };
    (host, w.id)
}

#[test]
fn a_host_in_the_servers_own_process_teleports_and_a_guest_is_refused_live_and_in_replay() {
    let dir = std::env::temp_dir().join(format!("server-host-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let log = dir.join("inputs.log");
    let spawns = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0]]
        .map(|pos| Spawn { pos, facing: 0.0 })
        .to_vec();
    let running = server::start(Config {
        spawns,
        tick_threads: 1,
        record: Some(log.clone()),
        ..Config::default()
    })
    .expect("a server");
    let (host, host_id) = host_joins(&running);
    let mut guest = Client::join(running.addr(), "Guest");
    guest.records_until(40, |g| *g == Got::Appear(host_id));
    host.send(teleport_to(1000, [500.0, 0.0, 0.0]))
        .expect("sent");
    guest
        .stream
        .write_all(&teleport_to(1000, [510.0, 0.0, 0.0]))
        .expect("sent");
    let told = guest.records_until(40, |g| matches!(g, Got::Correct(..)));
    assert!(told.contains(&Got::Correct(1, Why::Teleport)), "{told:?}");
    guest.claim(1, 1500, [13.0, 0.0, 0.0]);
    let mut ran_on = Vec::new();
    ClientMessage::Claim(Claim {
        ack: 0,
        movement: Movement {
            time: 1500,
            flags: flags::FORWARD,
            pos: [503.0, 0.0, 0.0],
            ..Movement::default()
        },
    })
    .write(&mut ran_on);
    host.send(ran_on).expect("sent");
    let batches = host_frames(&host, Duration::from_millis(300), |_| false)
        .iter()
        .filter(|f| matches!(ServerMessage::read(f), Ok(ServerMessage::Batch(_))))
        .count();
    assert!(batches > 0, "the host's batches stopped");
    let summary = running.stop().expect("a clean stop");
    assert_eq!(
        summary.refused,
        [0, 0, 0, 0, 0, 0, 1],
        "only the guest's teleport: each runs on from where the server put it"
    );

    let how = Replay {
        threads: 1,
        order: InputOrder::Canonical,
        keep_refusals: true,
        replicate: Replicate::No,
    };
    let r = server::replay(&log, &how).expect("a replay");
    assert_eq!(r.first_mismatch, None);
    let refused: Vec<(u32, Why)> = r.refusals.iter().map(|x| (x.id, x.why)).collect();
    assert_eq!(refused, [(guest.id, Why::Teleport)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_server_whose_tick_fails_closes_every_connection_it_would_have_admitted() {
    let running = server::start(Config {
        tick_threads: 1,
        record: Some(
            std::env::temp_dir()
                .join("no-such-dir-for-a-log")
                .join("inputs.log"),
        ),
        ..Config::default()
    })
    .expect("a server");
    let host = running.host_joins();
    let _ = host.send(hello("Host"));
    let mut guest = TcpStream::connect(running.addr().expect("a listener")).expect("connect");
    guest
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("a timeout");
    let _ = guest.write_all(&hello("Guest"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let host_let_go = loop {
        match host.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            _ if Instant::now() > deadline => break false,
            _ => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    assert!(host_let_go, "the host still waits for a welcome");
    let mut buf = [0u8; 64];
    assert!(
        matches!(guest.read(&mut buf), Ok(0) | Err(_)),
        "the guest still waits for a welcome"
    );
    assert!(running.wait().is_err(), "the tick's failure is lost");
}
