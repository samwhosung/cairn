use std::io;

use rayon::ThreadPool;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::net::{Outbox, Shared};
use crate::serve::Config;
use crate::sim::{Batches, Sim};
use crate::stats::TickStats;
use crate::world::{InputOrder, Refusal, Stamped};

/// A world ticked on its caller's clock, its clients in the same process: nothing sleeps and
/// nothing waits on a socket, so a run goes as fast as its ticks.
pub struct Stepper {
    sim: Sim,
    pool: ThreadPool,
    order: InputOrder,
    clients: Shared,
    batches: bool,
}

/// A client's end of its connection to a [`Stepper`]: the frames the server sent it, in order.
pub struct Link {
    frames: UnboundedReceiver<Vec<u8>>,
    read: Box<dyn Fn(usize) + Send + Sync>,
    behind: Box<dyn Fn(u32) + Send + Sync>,
}

impl Link {
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        self.frames.try_recv().ok()
    }

    /// Tells the server the client has taken in `bytes` of what it was sent, as its socket's reads
    /// would.
    pub fn taken_in(&self, bytes: usize) {
        (self.read)(bytes);
    }

    /// Tells the server how many ticks behind the client is, as its report of the latest tick it
    /// has seen would.
    pub fn behind_by(&self, ticks: u32) {
        (self.behind)(ticks);
    }
}

impl Stepper {
    pub fn new(cfg: &Config, order: InputOrder) -> io::Result<Self> {
        cfg.check()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cfg.tick_threads)
            .thread_name(|i| format!("tick-{i}"))
            .build()
            .map_err(io::Error::other)?;
        let sim = Sim::new(
            cfg.spawns.clone(),
            cfg.rules,
            cfg.view,
            cfg.map,
            cfg.tick_ms,
        );
        Ok(Self {
            sim,
            pool,
            order,
            clients: Shared::new(),
            batches: true,
        })
    }

    pub fn pool(&self) -> &ThreadPool {
        &self.pool
    }

    /// The tick the next [`Stepper::tick`] runs.
    pub fn next_tick(&self) -> u32 {
        self.sim.world().tick()
    }

    pub fn keep_refusals(&mut self) {
        self.sim.keep_refusals();
    }

    /// Builds no client's batch from now on, as a replay that only checks the world does.
    pub fn without_batches(&mut self) {
        self.batches = false;
    }

    /// Opens connection `conn`: once its join is admitted, the server's frames for it go to the
    /// returned link.
    pub fn connect(&self, conn: u32) -> Link {
        let (outbox, frames) = Outbox::channel();
        let link = Link {
            frames,
            read: Box::new(outbox.on_written()),
            behind: Box::new(outbox.behind_by()),
        };
        self.clients.hold_outbox(conn, outbox);
        link
    }

    /// Runs one tick on `inputs`, which must be sorted by connection and then in the order each
    /// connection sent them, and on the leave of any client the last tick gave up on.
    pub fn tick(&mut self, inputs: &[Stamped]) -> TickStats {
        let batches = if self.batches {
            Batches::Send(&self.clients)
        } else {
            Batches::Skip
        };
        let given_up = self.clients.take_inputs();
        if given_up.is_empty() {
            return self.sim.tick(&self.pool, inputs, self.order, batches);
        }
        let mut all = [inputs, &given_up].concat();
        all.sort_by_key(|s| (s.conn, s.nth));
        self.sim.tick(&self.pool, &all, self.order, batches)
    }

    /// Every claim refused since the last call, when [`Stepper::keep_refusals`] asked for them.
    pub fn take_refusals(&mut self) -> Vec<Refusal> {
        self.sim.take_refusals()
    }
}
