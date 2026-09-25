use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::{Anim, Engine, Game, Id, Knobs, KnobsFile, Line, Record, Shows, Spot, Tables, Tick};

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
    /// What the players among them that come back had saved, encoded, by number.
    pub restored: &'a [(u32, Vec<u8>)],
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stages {
    pub rules: Took,
    pub deliver: Took,
    pub record: Took,
}

/// A game running inside the server, whatever the game.
pub trait Hosted: Send + Sync {
    fn name(&self) -> &'static str;

    fn tick(&mut self, turn: &Turn<'_>);

    /// This tick's orders for the players' bodies, by number.
    fn orders(&self) -> &[(u32, BodyOrder)];

    fn record(&self) -> &Record;

    /// What this tick had the players' bodies show.
    fn shows(&self) -> &Shows;

    /// The pose a player's body holds now.
    fn held(&self, n: u32) -> Option<Anim>;

    fn tables(&self) -> &Tables;

    /// A player's sent fields, encoded.
    fn shown(&self, n: u32) -> Option<&[u8]>;

    fn hash(&self) -> u64;

    /// Every row's saved fields, encoded: a full scan.
    fn saved(&self) -> BTreeMap<Id, Vec<u8>>;

    /// What the rules counted, since the game started.
    fn counts(&self) -> &BTreeMap<&'static str, i64>;

    fn took(&self) -> Stages;
}

type Start = dyn Fn(u32, Delivery) -> Box<dyn Hosted> + Send + Sync;

/// A game with its knobs read, ready to start in a server.
#[derive(Clone)]
pub struct Loaded {
    name: &'static str,
    counts: &'static [&'static str],
    knobs: KnobsFile,
    over: Vec<Line>,
    seed: u64,
    tables: Tables,
    start: Arc<Start>,
}

impl Loaded {
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// The knobs file it read, its own when it was given none.
    pub fn knobs(&self) -> &KnobsFile {
        &self.knobs
    }

    pub fn overlay(&self) -> &[Line] {
        &self.over
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn tables(&self) -> &Tables {
        &self.tables
    }

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

/// Game `G` on the knobs file `base`, or on its own without one, with `over` laid on them.
pub fn load<G: Game>(base: Option<&KnobsFile>, over: &[Line], seed: u64) -> Result<Loaded, String> {
    let base = match base {
        Some(base) => base.clone(),
        None => KnobsFile::parse(G::KNOBS, &format!("{}'s own knobs", G::NAME))?,
    };
    let knobs = G::Knobs::read(&base, over)?;
    Ok(Loaded {
        name: G::NAME,
        counts: G::COUNTS,
        knobs: base,
        over: over.to_vec(),
        seed,
        tables: crate::engine::tables::<G>(),
        start: Arc::new(move |tick_ms, delivery| {
            Box::new(Engine::<G>::new(knobs.clone(), seed, tick_ms, delivery))
        }),
    })
}
