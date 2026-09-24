//! The world server: a 20 Hz bulk-synchronous tick that checks and relays movement.

mod grid;
mod log;
mod net;
mod replicate;
mod rules;
mod serve;
mod sim;
mod stats;
mod world;

use std::io;
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
use crate::net::Shared;
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

    /// Stops ticking, closes every connection, and returns what was measured.
    pub fn stop(self) -> io::Result<Summary> {
        self.shared.stop.store(true, Ordering::Relaxed);
        self.wait()
    }

    /// Waits until the window's [`Window::players`] have come and all have left, then drops every
    /// connection. Without a window it returns only on an error: use [`Running::stop`].
    pub fn wait(self) -> io::Result<Summary> {
        let summary = self
            .tick
            .join()
            .map_err(|_| io::Error::other("the tick thread panicked"))?;
        self.runtime.shutdown_background();
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
}

/// Replays the inputs recorded at `path` on `threads` threads, applying each entity's inputs in
/// `order`, and compares the world hash after every tick with the recorded one.
pub fn replay(
    path: &Path,
    threads: usize,
    order: InputOrder,
    keep_refusals: bool,
) -> io::Result<Replayed> {
    let mut log = LogReader::open(path)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
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
    if keep_refusals {
        sim.keep_refusals();
    }
    let mut out = Replayed::default();
    while let Some(logged) = log.next_tick()? {
        let st = sim.tick(&pool, &logged.inputs, order, Batches::Skip);
        if (st.tick != logged.tick || st.hash != logged.hash) && out.first_mismatch.is_none() {
            out.first_mismatch = Some(logged.tick);
        }
        out.ticks += 1;
        out.hash = st.hash;
        out.refusals.append(&mut sim.take_refusals());
    }
    Ok(out)
}
