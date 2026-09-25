use std::io;

use game::{Delivery, Hosted};
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

/// An entity in an observer's view: the slot its client knows it by, the game's state of it and
/// the pose the game has its body hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InView<'a> {
    pub slot: u16,
    pub id: u32,
    pub state: &'a [u8],
    pub pose: Option<u16>,
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
    pub fn new(cfg: &Config, order: InputOrder, delivery: Delivery) -> io::Result<Self> {
        cfg.check()?;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cfg.tick_threads)
            .thread_name(|i| format!("tick-{i}"))
            .build()
            .map_err(io::Error::other)?;
        let sim = Sim::new(
            cfg.spawns.clone(),
            cfg.limits,
            cfg.view,
            cfg.map,
            cfg.tick_ms,
        )
        .with_game(
            cfg.game
                .as_ref()
                .map(|g| g.start(u32::from(cfg.tick_ms), delivery)),
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

    pub fn game(&self) -> Option<&dyn Hosted> {
        self.sim.game()
    }

    pub fn placed_last_tick(&self) -> Vec<u32> {
        let world = self.sim.world();
        let tick = world.tick().wrapping_sub(1);
        (0..)
            .zip(world.before())
            .filter(|(_, b)| b.placed.is_some_and(|p| p.tick == tick))
            .map(|(id, _)| id)
            .collect()
    }

    /// What observer `id` has in view, by slot.
    pub fn in_view(&self, id: u32) -> Option<Vec<InView<'_>>> {
        let game = self.sim.game();
        let mut view: Vec<InView<'_>> = self
            .sim
            .in_view(id)?
            .into_iter()
            .map(|(slot, id)| {
                let state = game.and_then(|g| g.shown(id)).unwrap_or_default();
                let pose = game.and_then(|g| g.held(id)).map(|a| a.0);
                InView {
                    slot,
                    id,
                    state,
                    pose,
                }
            })
            .collect();
        view.sort_unstable_by_key(|v| v.slot);
        Some(view)
    }

    /// The pose the game has observer `id`'s own body hold.
    pub fn pose_of(&self, id: u32) -> Option<u16> {
        self.sim.game()?.held(id).map(|a| a.0)
    }

    /// The animations this tick played on the bodies in observer `id`'s view, by slot, and on its
    /// own, with no slot, in the order they were played.
    pub fn played_to(&self, id: u32) -> Vec<(Option<u16>, u16)> {
        let (Some(game), Some(view)) = (self.sim.game(), self.sim.in_view(id)) else {
            return Vec::new();
        };
        let slot_of = |n: u32| {
            if n == id {
                return Some(None);
            }
            let at = view.binary_search_by_key(&n, |&(_, v)| v).ok()?;
            Some(Some(view[at].0))
        };
        game.shows()
            .played
            .iter()
            .filter_map(|&(n, anim)| Some((slot_of(n)?, anim.0)))
            .collect()
    }

    pub fn saves_differ(&self) -> Option<String> {
        let game = self.sim.game()?;
        self.sim.saves().first_difference(&game.saved())
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
