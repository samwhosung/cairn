//! The world server: a 20 Hz bulk-synchronous tick that checks and relays movement, and keeps the
//! world in an SQLite file.

mod grid;
mod limits;
mod log;
mod net;
mod relays;
mod replicate;
mod save;
mod serve;
mod sim;
mod stats;
mod stepper;
mod world;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

pub use limits::{Limits, Why, ground_between};
pub use net::InProcess;
pub use protocol::Movement;
pub use replicate::{PastReach, Tier, View};
pub use save::{Commit, Keeping, Kept, Place, Player, Saving, default_world, read, scan};
pub use serve::{Config, Window};
pub use stats::{
    PHASES, SaveCost, Summary, TickStats, load_average, process_cpu_ns, thread_cpu_ns,
};
pub use stepper::{InView, Link, Stepper};
pub use world::{Input, InputOrder, Refusal, Spawn, Stamped};

use crate::log::LogReader;
use crate::net::Shared;

/// A server running on its own threads.
pub struct Running {
    addr: Option<SocketAddr>,
    shared: Arc<Shared>,
    tick: JoinHandle<io::Result<Summary>>,
    runtime: tokio::runtime::Runtime,
}

/// Opens the world's file, binds `cfg.addr`, if it names one, and starts ticking; refuses a view
/// past what a batch's positions reach ([`View::check`]), and a file the world cannot start from.
pub fn start(cfg: Config) -> io::Result<Running> {
    cfg.check()?;
    let opened = cfg.open_world()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cfg.io_threads.max(1))
        .thread_name("conn")
        .enable_io()
        .build()?;
    let shared = Arc::new(Shared::new());
    let addr = match cfg.addr {
        Some(addr) => {
            let listener = runtime.block_on(tokio::net::TcpListener::bind(addr))?;
            let bound = listener.local_addr()?;
            runtime.spawn(net::accept(listener, shared.clone()));
            Some(bound)
        }
        None => None,
    };
    let admitting = StopsAdmitting(shared.clone());
    let tick = std::thread::Builder::new()
        .name("world".into())
        .spawn(move || serve::run(&cfg, &admitting.0, opened))?;
    Ok(Running {
        addr,
        shared,
        tick,
        runtime,
    })
}

struct StopsAdmitting(Arc<Shared>);

impl Drop for StopsAdmitting {
    fn drop(&mut self) {
        self.0.stop_admitting();
    }
}

impl Running {
    pub fn addr(&self) -> Option<SocketAddr> {
        self.addr
    }

    /// Opens the host's connection; once its hello is in, the host may teleport.
    pub fn connect_host(&self) -> InProcess {
        net::connect_host(&self.shared, self.runtime.handle())
    }

    /// Stops ticking, closes every connection, and returns what was measured; like
    /// [`Running::wait`], not from async code.
    pub fn stop(self) -> io::Result<Summary> {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.wait()
    }

    /// Waits until its [`Config::window`] stops the server, then closes every connection and
    /// returns once the connections' tasks have ended, so not from async code. Without a window it
    /// returns only on an error: use [`Running::stop`].
    pub fn wait(self) -> io::Result<Summary> {
        let summary = self
            .tick
            .join()
            .map_err(|_| io::Error::other("the tick thread panicked"))?;
        drop(self.runtime);
        summary
    }
}

#[derive(Clone, Debug, Default)]
pub struct Replayed {
    pub ticks: u32,
    pub hash: u64,
    /// The first tick whose world hash differs from the recorded one.
    pub first_mismatch: Option<u32>,
    /// The ticks that saved anything.
    pub saving: Vec<u32>,
    /// Every player's saved state after [`Replay::keeping_at`].
    pub kept: Option<Keeping>,
    /// Every refused claim or teleport, when the replay was asked to keep them.
    pub refusals: Vec<Refusal>,
    /// Every replayed tick summarized over the time it stands for, one tick's length each.
    /// Nothing counts as received, and batches count as sent when they are built.
    pub summary: Summary,
}

#[derive(Clone, Copy, Debug)]
pub struct Replay<'a> {
    pub threads: usize,
    pub order: InputOrder,
    pub keep_refusals: bool,
    pub replicate: Replicate<'a>,
    /// Whether the players' actions reach the game: without them, a control that the world
    /// depends on them.
    pub actions: bool,
    /// The tick after which to take every player's saved state.
    pub keeping_at: Option<u32>,
}

/// Whether a replay builds every client's batch, as a server with the default [`View`] builds
/// them for clients that keep up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replicate<'a> {
    No,
    Yes,
    /// Yes, and write the frames the first connection received to this file.
    Dumping(&'a Path),
}

/// Replays the inputs recorded at `path`, with the game and the players the run started with,
/// and compares the world hash after every tick with the recorded one.
pub fn replay(path: &Path, how: &Replay<'_>) -> io::Result<Replayed> {
    let mut log = LogReader::open(path)?;
    let game = match &log.header.game {
        Some(g) => Some(
            catalog::load(&g.name, Some(&g.knobs), &g.overlay, g.seed).map_err(io::Error::other)?,
        ),
        None => None,
    };
    let cfg = Config {
        tick_threads: how.threads,
        tick_ms: log.header.tick_ms,
        map: log.header.map,
        spawns: log.header.spawns.clone(),
        limits: Limits {
            check: log.header.check,
            ..Limits::default()
        },
        game,
        ..Config::default()
    };
    let players = std::mem::take(&mut log.header.players);
    let mut stepper = Stepper::starting(&cfg, players, how.order)?;
    if how.keep_refusals {
        stepper.keep_refusals();
    }
    let mut dump = match how.replicate {
        Replicate::Dumping(path) => Some((BufWriter::new(File::create(path)?), stepper.connect(0))),
        Replicate::Yes => None,
        Replicate::No => {
            stepper.without_batches();
            None
        }
    };
    let (mut out, mut ticks) = (Replayed::default(), Vec::new());
    let cpu = process_cpu_ns();
    while let Some(mut logged) = log.next_tick()? {
        if !how.actions {
            logged
                .inputs
                .retain(|s| !matches!(s.input, world::Input::Action(_)));
        }
        let st = stepper.tick(&logged.inputs);
        if st.saved_rows > 0 {
            out.saving.push(st.tick);
        }
        if how.keeping_at == Some(st.tick) {
            out.kept = Some(stepper.keeping());
        }
        if (st.tick != logged.tick || st.hash != logged.hash) && out.first_mismatch.is_none() {
            out.first_mismatch = Some(logged.tick);
        }
        out.ticks += 1;
        out.hash = st.hash;
        out.refusals.append(&mut stepper.take_refusals());
        ticks.push(st);
        if let Some((file, link)) = &mut dump {
            while let Some(frame) = link.next_frame() {
                file.write_all(&frame)?;
                link.taken_in(frame.len());
            }
        }
    }
    if let Some((mut file, _)) = dump {
        file.flush()?;
    }
    let secs = f64::from(out.ticks) * f64::from(log.header.tick_ms) / 1000.0;
    out.summary = Summary::of(&ticks, how.threads, secs, 0, process_cpu_ns() - cpu);
    Ok(out)
}
