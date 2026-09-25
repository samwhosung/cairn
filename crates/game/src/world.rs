use std::any::{Any, TypeId};

use crate::space::Space;
use crate::{Game, Id, Kind, Spot, Table, Tick, dice};

/// Last tick's world, as every rule reads it. A rule holds it only as `&World`, so it can change
/// no row but its own, and no change it makes escapes the record.
pub struct World<'a, G: Game> {
    pub(crate) tick: Tick,
    pub(crate) tick_ms: u32,
    pub(crate) seed: u64,
    pub(crate) knobs: &'a G::Knobs,
    pub(crate) tables: &'a [Box<dyn Any + Send + Sync>],
    pub(crate) kinds: &'a [TypeId],
    pub(crate) space: &'a Space,
}

impl<'a, G: Game> World<'a, G> {
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// The first tick at least `ms` after this one.
    pub fn after_ms(&self, ms: u32) -> Tick {
        self.tick.saturating_add(ms.div_ceil(self.tick_ms.max(1)))
    }

    pub fn knobs(&self) -> &'a G::Knobs {
        self.knobs
    }

    pub fn table<K: Kind<G>>(&self) -> Option<&'a Table<K>> {
        let kind = self.kinds.iter().position(|&k| k == TypeId::of::<K>())?;
        self.tables[kind].downcast_ref()
    }

    /// Row `id` if it is of kind `K` and still in the world.
    pub fn row<K: Kind<G>>(&self, id: Id) -> Option<&'a K> {
        if *self.kinds.get(usize::from(id.kind))? != TypeId::of::<K>() {
            return None;
        }
        self.tables[usize::from(id.kind)]
            .downcast_ref::<Table<K>>()?
            .get(id.n)
    }

    pub fn player(&self, id: Id) -> Option<&'a G::Player> {
        self.row(id)
    }

    /// Where a player's body stood at the end of last tick; `None` once it has left.
    pub fn body(&self, id: Id) -> Option<Spot> {
        id.is_player().then(|| self.space.body(id.n)).flatten()
    }

    /// Where a player's body first stood.
    pub fn spawn(&self, id: Id) -> Option<Spot> {
        id.is_player().then(|| self.space.spawn(id.n)).flatten()
    }

    /// Each player whose body stood within `r` of `at` on the ground at the end of last tick, in
    /// an order that depends only on where they stood.
    pub fn near(&self, at: [f32; 3], r: f32, mut f: impl FnMut(Id, Spot)) {
        self.space.near(at, r, |n, spot| f(Id::player(n), spot));
    }

    /// A roll for row `id`: the same for the same seed, tick, row and `salt`, and no other.
    pub fn roll(&self, id: Id, salt: u32) -> u64 {
        dice::roll(self.seed, self.tick, id, salt)
    }

    /// A roll in `lo..=hi`, or `lo` when the range is empty.
    pub fn range(&self, id: Id, salt: u32, lo: u32, hi: u32) -> u32 {
        dice::within(self.roll(id, salt), lo, hi)
    }

    /// A roll true with probability `p`.
    pub fn chance(&self, id: Id, salt: u32, p: f64) -> bool {
        dice::chance(self.roll(id, salt), p)
    }
}
