use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::{Engine, Game, Id, Knobs, Line, Record, Spot, Tick, lines};

/// The order each row applies a round's letters in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Delivery {
    /// Sorted by sender and then by message.
    #[default]
    Canonical,
    /// The reverse of the canonical order: a control that a world depends on it.
    Reversed,
}

/// What the game asks of a player's body in a tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyOrder {
    pub place: Option<Spot>,
    pub root: Option<bool>,
}

/// What the server hands the game each tick.
pub struct Turn<'a> {
    pub tick: Tick,
    /// Bodies that joined since the last tick, each numbered and where it stands.
    pub joined: &'a [(u32, Spot)],
    /// Every player's body as last tick left it, by number; `None` for one that left.
    pub bodies: &'a [Option<Spot>],
    /// The players' actions, by number and then in the order each sent them.
    pub actions: &'a [(u32, u32)],
    /// The calling thread's CPU time, in nanoseconds.
    pub cpu_ns: fn() -> u64,
}

/// What a stage of the tick took: its CPU in all and in its largest task, and its wall time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Took {
    pub cpu_ns: u64,
    pub largest_ns: u64,
    pub wall_ns: u64,
}

/// A game running inside the server, whatever the game.
pub trait Hosted: Send {
    fn name(&self) -> &'static str;

    fn tick(&mut self, turn: &Turn<'_>);

    /// This tick's orders for the players' bodies, by number.
    fn orders(&self) -> &[(u32, BodyOrder)];

    fn record(&self) -> &Record;

    /// A player's sent fields, encoded, and the tick they last changed.
    fn shown(&self, n: u32) -> Option<(&[u8], Tick)>;

    fn hash(&self) -> u64;

    /// Every row's saved fields, encoded: a full scan.
    fn saved(&self) -> BTreeMap<Id, Vec<u8>>;

    /// What the rules counted, since the game started.
    fn counts(&self) -> &BTreeMap<&'static str, i64>;

    /// The CPU the last tick's stages took: stepping, delivering, recording.
    fn took(&self) -> [Took; 3];
}

type Start = dyn Fn(u32, Delivery) -> Box<dyn Hosted> + Send + Sync;

/// A game with its knobs read, ready to start in a server.
#[derive(Clone)]
pub struct Loaded {
    name: &'static str,
    counts: &'static [&'static str],
    start: Arc<Start>,
}

impl Loaded {
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// What its rules count.
    pub fn counts(&self) -> &'static [&'static str] {
        self.counts
    }

    pub fn start(&self, tick_ms: u32, delivery: Delivery) -> Box<dyn Hosted> {
        (self.start)(tick_ms, delivery)
    }
}

impl fmt::Debug for Loaded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Loaded({})", self.name)
    }
}

/// Game `G` on `base`, a knobs file's lines and the file's name, or on its own knobs without one,
/// with `over` laid on them.
pub fn load<G: Game>(
    base: Option<(&[Line], &str)>,
    over: &[Line],
    seed: u64,
) -> Result<Loaded, String> {
    let own_file = format!("{}'s own knobs", G::NAME);
    let knobs = if let Some((base, file)) = base {
        G::Knobs::read(base, file, over)?
    } else {
        G::Knobs::read(&lines(G::KNOBS, &own_file)?, &own_file, over)?
    };
    Ok(Loaded {
        name: G::NAME,
        counts: G::COUNTS,
        start: Arc::new(move |tick_ms, delivery| {
            Box::new(Engine::<G>::new(knobs.clone(), seed, tick_ms, delivery))
        }),
    })
}
