//! The world server: a 20 Hz bulk-synchronous tick that checks and relays movement.

mod grid;
mod log;
mod net;
mod relays;
mod replicate;
mod rules;
mod serve;
mod sim;
mod stats;
mod world;

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

pub use protocol::Movement;
pub use replicate::{Tier, View};
pub use rules::{Rules, Why, ground_between};
pub use serve::{Config, Window};
pub use stats::{PHASES, Summary, load_average, process_cpu_ns, thread_cpu_ns};
pub use world::{InputOrder, Refusal, Spawn};

use crate::log::LogReader;
use crate::net::{Outbox, Shared};
use crate::sim::{Batches, Sim};

/// A server running on its own threads.
pub struct Running {
    addr: SocketAddr,
    shared: Arc<Shared>,
    tick: JoinHandle<io::Result<Summary>>,
    runtime: tokio::runtime::Runtime,
}

/// Binds `cfg.addr` and starts ticking.
pub fn start(cfg: Config) -> io::Result<Running> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cfg.io_threads.max(1))
        .thread_name("conn")
        .enable_io()
        .build()?;
    let listener = runtime.block_on(tokio::net::TcpListener::bind(cfg.addr))?;
    let addr = listener.local_addr()?;
    let shared = Arc::new(Shared::new());
    runtime.spawn(net::accept(listener, shared.clone()));
    let for_tick = shared.clone();
    let tick = std::thread::Builder::new()
        .name("world".into())
        .spawn(move || serve::run(&cfg, &for_tick))?;
    Ok(Running {
        addr,
        shared,
        tick,
        runtime,
    })
}

impl Running {
    pub fn addr(&self) -> SocketAddr {
        self.addr
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
    /// Every refused claim, when the replay was asked to keep them.
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

/// Replays the inputs recorded at `path` and compares the world hash after every tick with the
/// recorded one.
pub fn replay(path: &Path, how: &Replay<'_>) -> io::Result<Replayed> {
    let mut log = LogReader::open(path)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(how.threads)
        .build()
        .map_err(io::Error::other)?;
    let rules = Rules {
        check: log.header.check,
        ..Rules::default()
    };
    let mut sim = Sim::new(
        log.header.spawns.clone(),
        rules,
        View::default(),
        0,
        log.header.tick_ms,
    );
    if how.keep_refusals {
        sim.keep_refusals();
    }
    let clients = Shared::new();
    let mut dump = match how.replicate {
        Replicate::Dumping(path) => {
            let (outbox, rx) = Outbox::channel();
            let on_written = outbox.on_written();
            clients.hold_outbox(0, outbox);
            Some((BufWriter::new(File::create(path)?), rx, on_written))
        }
        Replicate::No | Replicate::Yes => None,
    };
    let batches = match how.replicate {
        Replicate::No => Batches::Skip,
        Replicate::Yes | Replicate::Dumping(_) => Batches::Send(&clients),
    };
    let (mut out, mut ticks) = (Replayed::default(), Vec::new());
    let cpu = process_cpu_ns();
    while let Some(logged) = log.next_tick()? {
        let st = sim.tick(&pool, &logged.inputs, how.order, batches);
        if (st.tick != logged.tick || st.hash != logged.hash) && out.first_mismatch.is_none() {
            out.first_mismatch = Some(logged.tick);
        }
        out.ticks += 1;
        out.hash = st.hash;
        out.refusals.append(&mut sim.take_refusals());
        ticks.push(st);
        if let Some((file, rx, on_written)) = &mut dump {
            while let Ok(frame) = rx.try_recv() {
                file.write_all(&frame)?;
                on_written(frame.len());
            }
        }
    }
    if let Some((mut file, ..)) = dump {
        file.flush()?;
    }
    let secs = f64::from(out.ticks) * f64::from(log.header.tick_ms) / 1000.0;
    out.summary = Summary::of(&ticks, how.threads, secs, 0, process_cpu_ns() - cpu);
    Ok(out)
}
