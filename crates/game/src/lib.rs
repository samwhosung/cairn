//! The rule API a game on the server is written on: kinds of rows in typed tables, rules that see the world as the last tick left it and change only the row they run for, letters between rows, timers, spawning and knobs, and the tick that runs them.
//!
//! Letters reach their rows sorted by target, sender and message, at most twice a tick, and a roll
//! depends only on the seed, the tick, the row and a salt, so the world comes out the same on any
//! number of threads.

mod bytes;
mod canon;
mod columns;
mod dice;
mod engine;
mod hosted;
mod knobs;
mod out;
mod record;
mod show;
mod space;
mod table;
mod world;

use std::fmt::Debug;
use std::hash::Hash;

pub use bytes::Bytes;
pub use columns::{Column, Columns, Field, Scalar, Schema, Sql, Tables, Value};
pub use engine::Engine;
pub use hosted::{BodyOrder, Delivery, Hosted, Loaded, Stages, Took, Turn, load};
pub use knobs::{Knob, Knobs, KnobsFile, Line};
pub use out::Out;
pub use record::Record;
pub use show::{Anim, Shows, anim};
pub use space::Spot;
pub use table::Table;
pub use world::World;

pub type Tick = u32;

/// A row's name: its kind, counted in the order the game declares them with players as kind 0,
/// and its number within the kind. A player's number is its body's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id {
    pub kind: u16,
    pub n: u32,
}

impl Id {
    pub const fn player(n: u32) -> Self {
        Self { kind: 0, n }
    }

    pub const fn is_player(self) -> bool {
        self.kind == 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Letter<M> {
    pub from: Id,
    pub msg: M,
}

pub trait Game: Sized + Send + Sync + 'static {
    const NAME: &'static str;
    /// The knobs file the game runs on unless given another; it sets every key.
    const KNOBS: &'static str;
    /// What its rules count with [`Out::count`], each reported even while it stands at 0.
    const COUNTS: &'static [&'static str] = &[];
    type Knobs: Knobs;
    type Msg: Clone + Ord + Hash + Debug + Send + Sync + 'static;
    type Player: Kind<Self>;

    /// Adds the game's other kinds, in order: the first added is kind 1.
    fn kinds(kinds: &mut Kinds<Self>) {
        let _ = kinds;
    }

    /// The row a player's body gets as it joins, from what it saved when it comes back.
    fn join(id: Id, saved: Option<Saved<Self>>, w: &World<'_, Self>) -> Self::Player;

    /// The letter a player's action becomes, sent to its own row before the row steps.
    fn action(number: u32) -> Option<Self::Msg>;
}

pub type Saved<G> = <<G as Game>::Player as Kind<G>>::Saved;

/// A kind of row. Every field is part of the world, and the world's hash reads them all.
pub trait Kind<G: Game>: Clone + PartialEq + Hash + Debug + Send + Sync + 'static {
    /// What observers of a row are sent.
    type Sent: Bytes + PartialEq;
    type Saved: Columns;

    fn sent(&self) -> Self::Sent;

    fn saved(&self) -> Self::Saved;

    /// Runs in the tick the row first appears, then in each tick asked for with
    /// [`Out::wake_at`]; a row that asks for none sleeps until its next letter.
    fn step(id: Id, me: &mut Self, w: &World<'_, G>, out: &mut Out<G>) {
        let _ = (id, me, w, out);
    }

    /// Runs once a round for a row with letters, all of that round's in order.
    fn apply(id: Id, me: &mut Self, mail: &[Letter<G::Msg>], w: &World<'_, G>, out: &mut Out<G>) {
        let _ = (id, me, mail, w, out);
    }
}

/// The kinds a game keeps besides its players, as [`Game::kinds`] adds them.
pub struct Kinds<G: Game> {
    pub(crate) tables: Vec<table::Pair<G>>,
}

impl<G: Game> Kinds<G> {
    /// # Panics
    /// If `K` is the player's row or was added before.
    pub fn add<K: Kind<G>>(&mut self) -> &mut Self {
        let pair = table::Pair::of::<K>();
        assert!(
            self.tables.iter().all(|t| t.kind != pair.kind),
            "{} is a kind of {} twice",
            std::any::type_name::<K>(),
            G::NAME
        );
        self.tables.push(pair);
        self
    }
}
