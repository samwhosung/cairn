use std::io;

use game::{Delivery, Hosted};
use protocol::Whose;
use rayon::ThreadPool;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::net::{Outbox, Shared};
use crate::save::{Keeping, Player, Roster, scan};
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

/// An entity in an observer's view: the slot its client knows it by, the game's state of it, and
/// what the game has its body hold and idle in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InView<'a> {
    pub slot: u16,
    pub id: u32,
    pub state: &'a [u8],
    pub pose: Option<u16>,
    pub idle: Option<u16>,
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

    /// Whether the server has let the connection go and every frame it sent has been taken.
    pub fn closed(&self) -> bool {
        self.frames.is_closed() && self.frames.is_empty()
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
    /// A world on `cfg`, from its file when it names one, saving to it; each tick's results
    /// reach the links once its changes are committed, unless `cfg.saving` lets them out early.
    pub fn new(cfg: &Config, order: InputOrder, delivery: Delivery) -> io::Result<Self> {
        cfg.check()?;
        let opened = cfg.open_world()?;
        Self::with(cfg.sim(opened, delivery)?, cfg, order)
    }

    /// A world on `cfg` that knows `players` and keeps no file.
    pub fn starting(cfg: &Config, players: Vec<Player>, order: InputOrder) -> io::Result<Self> {
        let roster = Roster::new(players, cfg.map, cfg.tick_ms);
        let sim = cfg
            .sim(None, Delivery::Canonical)?
            .with_roster(roster, None);
        Self::with(sim, cfg, order)
    }

    fn with(sim: Sim, cfg: &Config, order: InputOrder) -> io::Result<Self> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(cfg.tick_threads)
            .thread_name(|i| format!("tick-{i}"))
            .build()
            .map_err(io::Error::other)?;
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

    /// Where body `id` stands as the last tick left it, while it is in the world.
    pub fn body(&self, id: u32) -> Option<game::Spot> {
        let b = self.sim.world().before().get(id as usize)?;
        b.present.then_some(game::Spot {
            pos: b.movement.pos,
            facing: b.movement.facing,
        })
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
                let idle = game.and_then(|g| g.idling(id)).map(|a| a.0);
                InView {
                    slot,
                    id,
                    state,
                    pose,
                    idle,
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

    /// What the game has observer `id`'s own body idle in.
    pub fn idle_of(&self, id: u32) -> Option<u16> {
        self.sim.game()?.idling(id).map(|a| a.0)
    }

    /// The animations this tick played on the bodies in observer `id`'s view and on its own, by
    /// body and then in the order its rules played them.
    pub fn played_to(&self, id: u32) -> Vec<(Whose, u16)> {
        let (Some(game), Some(view)) = (self.sim.game(), self.sim.in_view(id)) else {
            return Vec::new();
        };
        let whose = |n: u32| {
            if n == id {
                return Some(Whose::Own);
            }
            let at = view.binary_search_by_key(&n, |&(_, v)| v).ok()?;
            Some(Whose::Slot(view[at].0))
        };
        game.shows()
            .played
            .iter()
            .filter_map(|&(n, anim)| Some((whose(n)?, anim.0)))
            .collect()
    }

    /// Every player's saved state as the world has it: a full scan of the game's rows, and where
    /// each last stood as saved.
    pub fn keeping(&self) -> Keeping {
        self.sim.keeping()
    }

    /// Where the world's file differs from the world's saved state, by a full scan of each, after
    /// the last tick, whose changes [`Stepper::tick`] waited on unless the writer lets results
    /// out early; a scan that fails is a difference, and a world that keeps no file has none.
    pub fn file_differs(&self) -> Option<String> {
        let writer = self.sim.writer()?;
        let schema = self.sim.game().and_then(|g| g.tables().players);
        let file = match scan(writer.path(), schema.as_ref()) {
            Ok(file) => file,
            Err(e) => return Some(e),
        };
        first_difference(&self.sim.keeping(), &file)
    }

    /// Saves where every player stands, commits every change and closes the world's file.
    pub fn finish(mut self) -> Result<(), String> {
        self.sim.stop();
        self.sim
            .take_writer()
            .map_or(Ok(()), |w| w.finish().map(drop))
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
        let mut st = if given_up.is_empty() {
            self.sim.tick(&self.pool, inputs, self.order, batches)
        } else {
            let mut all = [inputs, &given_up].concat();
            all.sort_by_key(|s| (s.conn, s.nth));
            self.sim.tick(&self.pool, &all, self.order, batches)
        };
        self.sim.release();
        if let Some(writer) = self.sim.writer() {
            let waited = writer
                .wait_released(st.tick)
                .unwrap_or_else(|why| panic!("the world's file: {why}"));
            st.wait_ns = waited.as_nanos() as u64;
            st.commit = writer
                .take_commits()
                .into_iter()
                .find(|c| c.tick == st.tick);
        }
        st
    }

    pub fn world_file(&self) -> Option<&std::path::Path> {
        self.sim.writer().map(crate::save::Writer::path)
    }

    /// Every claim refused since the last call, when [`Stepper::keep_refusals`] asked for them.
    pub fn take_refusals(&mut self) -> Vec<Refusal> {
        self.sim.take_refusals()
    }
}

impl Drop for Stepper {
    fn drop(&mut self) {
        self.sim.stop();
    }
}

fn first_difference(world: &Keeping, file: &Keeping) -> Option<String> {
    for (name, kept) in world {
        match file.get(name) {
            None => return Some(format!("{name} is missing from the file")),
            Some(f) if f.saved != kept.saved => {
                let (a, b) = (&f.saved, &kept.saved);
                return Some(format!("{name}: the file saved {a:?}, and the world {b:?}"));
            }
            Some(f) if f.place != kept.place => {
                let (a, b) = (&f.place, &kept.place);
                return Some(format!(
                    "{name}: the file has it at {a:?}, and the world {b:?}"
                ));
            }
            Some(_) => {}
        }
    }
    file.keys()
        .find(|name| !world.contains_key(*name))
        .map(|name| format!("{name} is in the file and not the world"))
}
