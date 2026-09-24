//! The world server: a 20 Hz bulk-synchronous tick that checks and relays movement.
//!
//! Each tick reads the inputs that arrived since the last one, steps every entity from the last
//! tick's world into this one's in parallel (an entity writes only itself), rebuilds the spatial
//! grid, then builds every player's batch in parallel. The same inputs give the same world on
//! any number of threads. [`start`] serves it over TCP on its own threads, so a client can run
//! it in-process and reach it over loopback just as it reaches a remote one.

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

pub use replicate::{Tier, View};
pub use rules::Rules;
pub use serve::{Config, Window};
pub use stats::{PHASES, Summary, load_average, process_cpu_ns, thread_cpu_ns};
pub use world::{Order, Spawn};

use crate::log::LogReader;
use crate::net::Shared;
use crate::sim::Sim;

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

    /// Waits for the measured window to end, then closes every connection.
    pub fn wait(self) -> io::Result<Summary> {
        let summary = self
            .tick
            .join()
            .map_err(|_| io::Error::other("the tick thread panicked"))?;
        self.runtime.shutdown_background();
        summary
    }
}

/// What replaying a recorded run gave.
#[derive(Clone, Copy, Debug, Default)]
pub struct Replayed {
    pub ticks: u32,
    pub hash: u64,
    /// The first tick whose world hash differs from the recorded one.
    pub first_mismatch: Option<u32>,
}

/// Replays the inputs recorded at `path` on `threads` threads, applying each entity's inputs in
/// `order`, and compares the world hash after every tick with the recorded one.
pub fn replay(path: &Path, threads: usize, order: Order) -> io::Result<Replayed> {
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
    let mut out = Replayed::default();
    while let Some(logged) = log.next_tick()? {
        let st = pool.install(|| sim.tick(&logged.inputs, order, None, false));
        if (st.tick != logged.tick || st.hash != logged.hash) && out.first_mismatch.is_none() {
            out.first_mismatch = Some(logged.tick);
        }
        out.ticks += 1;
        out.hash = st.hash;
    }
    Ok(out)
}
