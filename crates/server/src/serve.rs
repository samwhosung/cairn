use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::log::{Header, LogWriter};
use crate::net::Shared;
use crate::replicate::View;
use crate::rules::Rules;
use crate::sim::Sim;
use crate::stats::{Summary, TickStats, process_cpu_ns};
use crate::world::{Order, Spawn};

/// How a server runs.
#[derive(Clone, Debug)]
pub struct Config {
    pub addr: SocketAddr,
    /// Threads that run a tick's phases.
    pub threads: usize,
    /// Threads that serve the connections.
    pub io_threads: usize,
    pub tick_ms: u16,
    /// The `Map.dbc` id players are welcomed onto.
    pub map: u32,
    /// Where players join, in turn.
    pub spawns: Vec<Spawn>,
    pub rules: Rules,
    pub view: View,
    /// Where to write every tick's inputs and world hash, for replay.
    pub record: Option<PathBuf>,
    /// When to measure and stop; without one the server runs until stopped.
    pub window: Option<Window>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            threads: std::thread::available_parallelism().map_or(1, usize::from),
            io_threads: 4,
            tick_ms: 50,
            map: 0,
            spawns: Vec::new(),
            rules: Rules::default(),
            view: View::default(),
            record: None,
            window: None,
        }
    }
}

/// Once `players` are in, wait `settle` ticks and measure `measure` ticks; then run on until
/// every player has left.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub players: u32,
    pub settle: u32,
    pub measure: u32,
}

/// Where a measured window began.
struct Mark {
    tick: usize,
    at: Instant,
    bytes_in: u64,
    cpu: u64,
}

impl Mark {
    fn now(tick: usize, shared: &Shared) -> Self {
        Self {
            tick,
            at: Instant::now(),
            bytes_in: shared.bytes_in.load(Ordering::Relaxed),
            cpu: process_cpu_ns(),
        }
    }

    fn summary(&self, ticks: &[TickStats], threads: usize, shared: &Shared) -> Summary {
        Summary::of(
            ticks.get(self.tick..).unwrap_or_default(),
            threads,
            self.at.elapsed().as_secs_f64(),
            shared.bytes_in.load(Ordering::Relaxed) - self.bytes_in,
            process_cpu_ns() - self.cpu,
        )
    }
}

/// Ticks in real time until stopped, or once the measured window is over, until every player
/// has left.
pub(crate) fn run(cfg: &Config, shared: &Shared) -> io::Result<Summary> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.threads)
        .thread_name(|i| format!("tick-{i}"))
        .build()
        .map_err(io::Error::other)?;
    let mut sim = Sim::new(
        cfg.spawns.clone(),
        cfg.rules,
        cfg.view,
        cfg.map,
        cfg.tick_ms,
    );
    let mut log = match &cfg.record {
        Some(path) => Some(LogWriter::create(
            path,
            &Header {
                tick_ms: cfg.tick_ms,
                check: cfg.rules.check,
                spawns: sim.world().spawns().to_vec(),
            },
        )?),
        None => None,
    };
    let period = Duration::from_millis(u64::from(cfg.tick_ms));
    let mut due = Instant::now();
    let mut ticks: Vec<TickStats> = Vec::new();
    let (mut mark, mut full_at) = (None::<Mark>, None::<usize>);
    let mut measured = None;
    while !shared.stop.load(Ordering::Relaxed) {
        due = (due + period).max(Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        let inputs = shared.take_inputs();
        let st = pool.install(|| sim.tick(&inputs, Order::Canonical, Some(shared), true));
        if let Some(log) = &mut log {
            log.tick(st.tick, &inputs, st.hash)?;
        }
        ticks.push(st);
        let n = ticks.len();
        match cfg.window {
            Some(_) if measured.is_some() => {
                if st.players == 0 {
                    break;
                }
            }
            Some(w) => {
                if full_at.is_none() && st.players >= w.players {
                    full_at = Some(n);
                }
                let start = full_at.map(|f| f + w.settle as usize);
                if mark.is_none() && start == Some(n) {
                    mark = Some(Mark::now(n, shared));
                }
                if start.is_some_and(|s| n >= s + w.measure as usize) {
                    measured = mark.take().map(|m| m.summary(&ticks, cfg.threads, shared));
                }
            }
            None if mark.is_none() && st.players > 0 => mark = Some(Mark::now(n, shared)),
            None => {}
        }
    }
    if let Some(log) = log {
        log.finish()?;
    }
    Ok(measured
        .or_else(|| mark.map(|m| m.summary(&ticks, cfg.threads, shared)))
        .unwrap_or_default())
}
