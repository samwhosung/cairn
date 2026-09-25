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
const KILLS: usize = 5;

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

/// What one run killed at a random moment left: the last tick a client was sent, the last tick
/// in the file, and whether the file holds the world as it stood then.
struct Killed {
    sent: u32,
    in_file: Option<u32>,
    file_is_the_world: bool,
    /// Ticks after the one in the file, up to the last one a client was sent, that saved anything.
    lost: Vec<u32>,
    /// Every fighter's kills and deaths in the file, summed.
    kills_deaths: (i64, i64),
}

impl Killed {
    fn say(&self) -> String {
        let (kills, deaths) = self.kills_deaths;
        format!(
            "a client was sent tick {}, the file holds tick {:?} ({kills} kills, {deaths} deaths \
             in all), the file is the world then: {}, ticks that saved and were sent after it: {:?}",
            self.sent, self.in_file, self.file_is_the_world, self.lost
        )
    }
}

fn run_and_kill(dir: &Path, round: usize, after: Duration, early: bool) -> Killed {
    let log = dir.join(format!("run-{round}.log"));
    let mut served = serve(dir, &log, early);
    let stop = Arc::new(AtomicBool::new(false));
    let fighters: Vec<JoinHandle<Option<u32>>> = (0..FIGHTERS)
        .map(|i| fighter(served.addr, format!("F{i}"), stop.clone()))
        .collect();
    std::thread::sleep(after);
    served.child.kill().expect("killed");
    served.child.wait().expect("reaped");
    stop.store(true, Ordering::Relaxed);
    let sent = fighters
        .into_iter()
        .filter_map(|f| f.join().expect("a fighter"))
        .max()
        .expect("a batch reached a client");
    let world = dir.join("world.sqlite");
    let asked = server::read(
        &world,
        &["SELECT value FROM world WHERE key = 'tick'".into()],
        Duration::from_secs(5),
    )
    .expect("the file reads");
    let in_file = asked.lines().nth(1).and_then(|v| v.parse().ok());
    let how = Replay {
        threads: 1,
        order: InputOrder::Canonical,
        keep_refusals: false,
        replicate: Replicate::No,
        actions: true,
        keeping_at: in_file,
    };
    let replayed = server::replay(&log, &how).expect("the log replays");
    assert_eq!(
        replayed.first_mismatch, None,
        "the run replays tick for tick"
    );
    let melee = catalog::load("melee", None, &[], 0).expect("melee");
    let file = server::scan(&world, melee.schemas()[0].as_ref()).expect("a full scan");
    let lost = replayed
        .saving
        .iter()
        .copied()
        .filter(|&t| in_file.is_none_or(|d| t > d) && t <= sent)
        .collect();
    let whole = |i: usize| -> i64 {
        file.values()
            .filter_map(|k| match k.saved.as_ref()?.get(i)? {
                game::Value::Integer(n) => Some(*n),
                _ => None,
            })
            .sum()
    };
    Killed {
        sent,
        in_file,
        file_is_the_world: replayed.kept.as_ref() == Some(&file),
        lost,
        kills_deaths: (whole(0), whole(1)),
    }
}

/// A killed server has lost no result a client was sent: every tick up to the last one a client
/// was sent that saved anything is in the file, and the file is the world as it stood at its
/// last tick. The control lets results out before their changes are durable: a kill loses one.
#[test]
fn a_server_killed_at_random_loses_nothing_a_client_was_sent() {
    let dir = scratch("held");
    let kills = std::env::var("CAIRN_KILLS").map_or(KILLS, |n| n.parse().expect("a count"));
    let mut rng = u64::from(std::process::id()) | 1;
    for round in 0..kills {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let after = Duration::from_millis(1500 + rng % 3000);
        let killed = run_and_kill(&dir, round, after, false);
        let said = killed.say();
        eprintln!("kill {round} after {after:?}: {said}");
        assert!(killed.file_is_the_world && killed.lost.is_empty(), "{said}");
    }
    let dir = scratch("early");
    for round in 0..2 {
        let killed = run_and_kill(&dir, round, Duration::from_millis(2000), true);
        let said = killed.say();
        eprintln!("control {round}: {said}");
        assert!(killed.file_is_the_world, "{said}");
        assert!(
            !killed.lost.is_empty(),
            "results let out early: a kill loses one"
        );
    }
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
