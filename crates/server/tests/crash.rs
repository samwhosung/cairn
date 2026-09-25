use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use protocol::{Appearance, ClientMessage, Frames, Hello, ServerMessage, VERSION};
use server::{InputOrder, Replay, Replicate};

const FIGHTERS: usize = 6;
const QUICK: &str = "swing_ms = 100\ndamage_min = 40\ndamage_max = 80\nrespawn_s = 1\n";
/// Melee's action that swings.
const SWING: u32 = 1;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("crash-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    std::fs::write(dir.join("quick.knobs"), QUICK).expect("an overlay");
    let ring: Vec<String> = (0..FIGHTERS)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / FIGHTERS as f32;
            let (x, y) = (1.5 * libm::cosf(a), 1.5 * libm::sinf(a));
            format!("{x} {y} 0 {}\n", a + std::f32::consts::PI)
        })
        .collect();
    let ring = ring.concat();
    std::fs::write(dir.join("ring.spawns"), ring).expect("spawns");
    dir
}

/// A fighter over TCP: it swings ten times a second and keeps the last tick it was sent.
fn fighter(addr: SocketAddr, name: String, stop: Arc<AtomicBool>) -> JoinHandle<Option<u32>> {
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_millis(10)))
            .ok()?;
        let mut out = Vec::new();
        let hello = Hello {
            version: VERSION,
            name,
            appearance: Appearance::default(),
        };
        ClientMessage::Hello(hello).write(&mut out);
        stream.write_all(&out).ok()?;
        let (mut frames, mut buf) = (Frames::default(), vec![0; 1 << 16]);
        let (mut seen, mut swing_at) = (None, Instant::now());
        while !stop.load(Ordering::Relaxed) {
            if Instant::now() >= swing_at {
                out.clear();
                ClientMessage::Action(SWING).write(&mut out);
                let _ = stream.write_all(&out);
                swing_at += Duration::from_millis(100);
            }
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => frames.extend(&buf[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                Err(_) => break,
            }
            while let Ok(Some(frame)) = frames.next_frame() {
                if let Ok(ServerMessage::Batch(b)) = ServerMessage::read(frame) {
                    seen = seen.max(Some(b.tick));
                }
            }
        }
        seen
    })
}

struct Served {
    child: Child,
    addr: SocketAddr,
}

fn serve(dir: &Path, log: &Path, early: bool) -> Served {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_server"));
    cmd.args([
        "--port",
        "0",
        "--threads",
        "2",
        "--io-threads",
        "1",
        "--game",
        "melee",
    ])
    .arg("--overlay")
    .arg(dir.join("quick.knobs"))
    .arg("--spawns")
    .arg(dir.join("ring.spawns"))
    .arg("--world")
    .arg(dir.join("world.sqlite"))
    .arg("--record")
    .arg(log)
    .stdout(Stdio::null())
    .stderr(Stdio::piped());
    if early {
        cmd.arg("--results-early");
    }
    let mut child = cmd.spawn().expect("the server starts");
    let mut lines = BufReader::new(child.stderr.take().expect("its stderr")).lines();
    let addr = lines
        .by_ref()
        .map_while(Result::ok)
        .find_map(|l| l.strip_prefix("serving on ")?.trim().parse().ok())
        .expect("the server says where it serves");
    std::thread::spawn(move || lines.for_each(drop));
    Served { child, addr }
}

/// A game's run replays tick for tick; without the players' actions it parts.
#[test]
fn a_games_run_replays_at_every_tick_and_parts_without_its_actions() {
    let dir = scratch("replay");
    let log = dir.join("run.log");
    let mut served = serve(&dir, &log, false);
    let stop = Arc::new(AtomicBool::new(false));
    let fighters: Vec<_> = (0..FIGHTERS)
        .map(|i| fighter(served.addr, format!("F{i}"), stop.clone()))
        .collect();
    std::thread::sleep(Duration::from_millis(1500));
    served.child.kill().expect("killed");
    served.child.wait().expect("reaped");
    stop.store(true, Ordering::Relaxed);
    for f in fighters {
        drop(f.join());
    }
    let replay = |actions| {
        let how = Replay {
            threads: 2,
            order: InputOrder::Canonical,
            keep_refusals: false,
            replicate: Replicate::No,
            actions,
            keeping_at: None,
        };
        server::replay(&log, &how).expect("the log replays")
    };
    let with = replay(true);
    assert!(with.ticks > 20 && with.first_mismatch.is_none(), "{with:?}");
    let without = replay(false);
    assert!(without.first_mismatch.is_some(), "the control parts");
    eprintln!(
        "{} ticks replayed alike; without the actions the world parts at tick {:?}",
        with.ticks, without.first_mismatch
    );
}
