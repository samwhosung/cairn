use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use protocol::{Appearance, ClientMessage, Frames, Hello, ServerMessage, VERSION, Welcome};
use server::{Config, InputOrder, Replay, Replicate};

fn join(addr: SocketAddr, name: &str) -> (TcpStream, Welcome) {
    let mut stream = TcpStream::connect(addr).expect("a connection");
    stream
        .set_read_timeout(Some(Duration::from_millis(20)))
        .expect("a timeout");
    let mut hello = Vec::new();
    ClientMessage::Hello(Hello {
        version: VERSION,
        name: name.into(),
        appearance: Appearance::default(),
    })
    .write(&mut hello);
    stream.write_all(&hello).expect("the hello sent");
    let (mut frames, mut buf) = (Frames::default(), vec![0; 1 << 16]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => frames.extend(&buf[..n]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => continue,
            Err(e) => panic!("reading: {e}"),
        }
        while let Ok(Some(frame)) = frames.next_frame() {
            if let Ok(ServerMessage::Welcome(w)) = ServerMessage::read(frame) {
                return (stream, w);
            }
        }
    }
    panic!("{name} was never welcomed");
}

/// Whether the server closes the stream within `wait`, whatever it sends first.
fn closed_within(stream: &mut TcpStream, wait: Duration) -> bool {
    let (deadline, mut buf) = (Instant::now() + wait, vec![0; 1 << 16]);
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return true,
        }
    }
    false
}

#[test]
fn a_player_whose_connection_died_gets_back_in_at_once_and_the_old_one_is_closed() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("takeover");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let log = dir.join("run.log");
    let cfg = Config {
        addr: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
        tick_threads: 2,
        io_threads: 1,
        game: Some(catalog::load("melee", None, &[], 1).expect("melee")),
        record: Some(log.clone()),
        ..Config::default()
    };
    let running = server::start(cfg).expect("a server");
    let addr = running.addr().expect("an address");
    let (mut dead, first) = join(addr, "Ada");
    let (mut again, second) = join(addr, "Ada");
    assert_eq!(second.id, first.id, "the same body, at once");
    assert!(
        closed_within(&mut dead, Duration::from_secs(5)),
        "the server lets the old connection go"
    );
    let (_, bo) = join(addr, "Bo");
    assert_ne!(bo.id, first.id);
    assert!(
        !closed_within(&mut again, Duration::from_millis(500)),
        "the control: a join under another name leaves Ada's connection be"
    );
    running.stop().expect("stopped");
    let how = Replay {
        threads: 1,
        order: InputOrder::Canonical,
        keep_refusals: false,
        replicate: Replicate::No,
        actions: true,
        keeping_at: None,
    };
    let replayed = server::replay(&log, &how).expect("the log replays");
    assert!(
        replayed.ticks > 0 && replayed.first_mismatch.is_none(),
        "the takeover replays tick for tick: {replayed:?}"
    );
}
