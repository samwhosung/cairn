use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use game::{Delivery, Loaded};

use crate::limits::Limits;
use crate::log::{Header, LogWriter, LoggedGame};
use crate::net::Shared;
use crate::replicate::View;
use crate::save::{Flush, Opened, Roster, Saving, Writer};
use crate::sim::{Batches, Sim};
use crate::stats::{SaveCost, Summary, TickStats, process_cpu_ns};
use crate::world::{InputOrder, Spawn};

const MAX_UNRELEASED_MS: u32 = 1000;

#[derive(Clone, Debug)]
pub struct Config {
    /// Where players connect over TCP; with none, only the host joins, from the same process.
    pub addr: Option<SocketAddr>,
    pub tick_threads: usize,
    pub io_threads: usize,
    pub tick_ms: u16,
    /// The `Map.dbc` id players are welcomed onto.
    pub map: u32,
    /// Where players join, in turn; with none, everyone joins at the map's origin.
    pub spawns: Vec<Spawn>,
    pub limits: Limits,
    pub view: View,
    /// The game whose rules the world runs; with none, it runs only movement.
    pub game: Option<Loaded>,
    /// The file the world starts from and saves each tick's changes to; with none, the world is
    /// kept only in memory.
    pub world: Option<PathBuf>,
    pub flush: Flush,
    pub saving: Saving,
    /// Where to write every tick's inputs and world hash, for replay.
    pub record: Option<PathBuf>,
    /// When to measure and stop; without one the server runs until stopped.
    pub window: Option<Window>,
}

impl Config {
    pub(crate) fn check(&self) -> io::Result<()> {
        self.view
            .check(&self.limits)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }

    pub(crate) fn open_world(&self) -> io::Result<Option<Opened>> {
        let Some(path) = &self.world else {
            return Ok(None);
        };
        let game = self.game.as_ref().map(|g| (g.name(), g.tables()));
        crate::save::open(path, game, self.flush)
            .map(Some)
            .map_err(io::Error::other)
    }

    pub(crate) fn sim(&self, opened: Option<Opened>, delivery: Delivery) -> io::Result<Sim> {
        let players = opened
            .as_ref()
            .map(|o| o.players.clone())
            .unwrap_or_default();
        let roster = Roster::new(players, self.map, self.tick_ms);
        let writer = opened.map(|o| Writer::start(o, self.saving)).transpose()?;
        let sim = Sim::new(
            self.spawns.clone(),
            self.limits,
            self.view,
            self.map,
            self.tick_ms,
        )
        .with_game(
            self.game
                .as_ref()
                .map(|g| g.start(u32::from(self.tick_ms), delivery)),
        )
        .with_roster(roster, writer);
        Ok(sim)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addr: Some(SocketAddr::from(([127, 0, 0, 1], 0))),
            tick_threads: std::thread::available_parallelism().map_or(1, usize::from),
            io_threads: 4,
            tick_ms: 50,
            map: 0,
            spawns: Vec::new(),
            limits: Limits::default(),
            view: View::default(),
            game: None,
            world: None,
            flush: Flush::Drive,
            saving: Saving::default(),
            record: None,
            window: None,
        }
    }
}

/// Once `players` are in, or `arrival` ticks after the server started with whoever is, wait
/// `settle` ticks and measure `measure` ticks; run on until every player has left or the `grace`
/// has run out, and if they leave sooner, report what was measured by then.
#[derive(Clone, Copy, Debug)]
pub struct Window {
    pub players: u32,
    pub arrival: u32,
    pub settle: u32,
    pub measure: u32,
    /// Ticks the players have to leave once the window has measured; then the server drops
    /// every connection still open.
    pub grace: u32,
}

struct WindowStart {
    tick: usize,
    at: Instant,
    bytes_in: u64,
    cpu: u64,
}

impl WindowStart {
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

pub(crate) fn run(cfg: &Config, shared: &Shared, opened: Option<Opened>) -> io::Result<Summary> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(cfg.tick_threads)
        .thread_name(|i| format!("tick-{i}"))
        .build()
        .map_err(io::Error::other)?;
    let mut sim = cfg.sim(opened, Delivery::Canonical)?;
    let mut log = cfg
        .record
        .as_ref()
        .map(|path| log(path, cfg, &sim))
        .transpose()?;
    let period = Duration::from_millis(u64::from(cfg.tick_ms));
    let mut due = Instant::now();
    let mut ticks: Vec<TickStats> = Vec::new();
    let (mut mark, mut crowd_in_at) = (None::<WindowStart>, None::<usize>);
    let (mut measured, mut measured_at, mut stayed) = (None, usize::MAX, 0);
    let mut arrived = 0;
    while !shared.stop.load(Ordering::Relaxed) {
        due = (due + period).max(Instant::now());
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        let inputs = shared.take_inputs();
        shared
            .latest_tick
            .store(sim.world().tick(), Ordering::Relaxed);
        let mut st = sim.tick(&pool, &inputs, InputOrder::Canonical, Batches::Send(shared));
        if let Some(log) = &mut log {
            log.tick(st.tick, &inputs, st.hash)?;
        }
        sim.release();
        if let Some(writer) = sim.writer() {
            if let Some(why) = writer.failed() {
                return Err(io::Error::other(why));
            }
            let most = (MAX_UNRELEASED_MS / u32::from(cfg.tick_ms.max(1))).max(1);
            if let Some(back) = st.tick.checked_sub(most) {
                let waited = writer.wait_released(back).map_err(io::Error::other)?;
                st.wait_ns = waited.as_nanos() as u64;
            }
        }
        ticks.push(st);
        if let Some(writer) = sim.writer() {
            for c in writer.take_commits() {
                if let Some(t) = ticks.get_mut(c.tick as usize) {
                    t.commit = Some(c);
                }
            }
        }
        let n = ticks.len();
        match cfg.window {
            Some(_) if crowd_in_at.is_some() && st.players == 0 => break,
            Some(w) if n >= measured_at.saturating_add(w.grace as usize) => {
                stayed = st.players;
                break;
            }
            Some(_) if measured.is_some() => {}
            Some(w) => {
                if crowd_in_at.is_none() && (st.players >= w.players || n >= w.arrival as usize) {
                    crowd_in_at = Some(n);
                    arrived = st.players;
                }
                let start = crowd_in_at.map(|f| f + w.settle as usize);
                if mark.is_none() && start == Some(n) {
                    mark = Some(WindowStart::now(n, shared));
                }
                if start.is_some_and(|s| n >= s + w.measure as usize) {
                    measured = mark
                        .take()
                        .map(|m| m.summary(&ticks, cfg.tick_threads, shared));
                    measured_at = n;
                }
            }
            None if mark.is_none() && st.players > 0 => mark = Some(WindowStart::now(n, shared)),
            None => {}
        }
    }
    if let Some(log) = log {
        log.finish()?;
    }
    stop(&mut sim, &mut ticks)?;
    if let Some(m) = &mut measured {
        let window = measured_at.saturating_sub(m.ticks)..measured_at;
        m.saving = SaveCost::of(ticks.get(window).unwrap_or_default());
    }
    let mut summary = measured
        .or_else(|| mark.map(|m| m.summary(&ticks, cfg.tick_threads, shared)))
        .unwrap_or_default();
    summary.ticks_after = ticks.len().saturating_sub(measured_at) as u32;
    summary.stayed = stayed;
    summary.players_arrived = arrived;
    summary.players_wanted = cfg.window.map_or(0, |w| w.players);
    Ok(summary)
}

fn log(path: &std::path::Path, cfg: &Config, sim: &Sim) -> io::Result<LogWriter> {
    let header = Header {
        tick_ms: cfg.tick_ms,
        check: cfg.limits.check,
        map: cfg.map,
        spawns: sim.world().spawns().to_vec(),
        game: cfg.game.as_ref().map(|g| LoggedGame {
            name: g.name().to_owned(),
            seed: g.seed(),
            knobs: g.knobs().clone(),
            overlay: g.overlay().to_vec(),
        }),
        players_at_start: sim.roster().players().cloned().collect(),
    };
    LogWriter::create(path, &header)
}

fn stop(sim: &mut Sim, ticks: &mut [TickStats]) -> io::Result<()> {
    sim.stop();
    if let Some(writer) = sim.take_writer() {
        for c in writer.finish().map_err(io::Error::other)? {
            if let Some(t) = ticks.get_mut(c.tick as usize) {
                t.commit = Some(c);
            }
        }
    }
    Ok(())
}
