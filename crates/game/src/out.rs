use std::any::{Any, TypeId};

use crate::engine::NEVER;
use crate::hosted::BodyOrder;
use crate::show::{BodyShow, Show};
use crate::{Anim, Game, Id, Kind, Letter, Outcome, Spot, Tick};

pub(crate) const ACT: u8 = 0;
pub(crate) const STEP: u8 = 1;
pub(crate) const ROUND: u8 = 2;

pub(crate) struct PendingSpawn {
    pub phase: u8,
    pub from: Id,
    pub seq: u32,
    pub kind: TypeId,
    pub row: Box<dyn Any + Send>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fate {
    Stays,
    Leaves,
}

/// What a rule does beyond writing its own row.
pub struct Out<G: Game> {
    phase: u8,
    me: Id,
    spawned: u32,
    wake: Tick,
    gone: bool,
    order: Option<BodyOrder>,
    pub(crate) letters: Vec<(Id, Letter<G::Msg>)>,
    pub(crate) spawns: Vec<PendingSpawn>,
    pub(crate) orders: Vec<(u32, u8, BodyOrder)>,
    pub(crate) shows: Vec<BodyShow>,
    pub(crate) counts: Vec<(&'static str, i64)>,
}

impl<G: Game> Out<G> {
    pub(crate) fn new(phase: u8) -> Self {
        Self {
            phase,
            me: Id::player(0),
            spawned: 0,
            wake: NEVER,
            gone: false,
            order: None,
            letters: Vec::new(),
            spawns: Vec::new(),
            orders: Vec::new(),
            shows: Vec::new(),
            counts: Vec::new(),
        }
    }

    pub(crate) fn begin(&mut self, me: Id) {
        self.me = me;
        self.spawned = 0;
    }

    pub(crate) fn end(&mut self, due: &mut Tick) -> Fate {
        *due = (*due).min(std::mem::replace(&mut self.wake, NEVER));
        if let Some(order) = self.order.take()
            && self.me.is_player()
        {
            self.orders.push((self.me.n, self.phase, order));
        }
        if std::mem::take(&mut self.gone) {
            Fate::Leaves
        } else {
            Fate::Stays
        }
    }

    /// Sends `msg` to the row `to`, which applies it in this tick's next round of delivery, or in
    /// the next tick's first once this tick's rounds are spent.
    pub fn send(&mut self, to: Id, msg: G::Msg) {
        self.letters.push((to, Letter { from: self.me, msg }));
    }

    /// Adds `row` to its kind at the tick's end, numbered after every row there is; it steps in
    /// the next tick. The rows spawned in a tick are numbered by who spawned them and then in the
    /// order each did, whatever thread ran it.
    pub fn spawn<K: Kind<G>>(&mut self, row: K) {
        self.spawns.push(PendingSpawn {
            phase: self.phase,
            from: self.me,
            seq: self.spawned,
            kind: TypeId::of::<K>(),
            row: Box::new(row),
        });
        self.spawned += 1;
    }

    /// Takes this row out of the world at the tick's end; a player's row stays.
    pub fn despawn(&mut self) {
        self.gone = !self.me.is_player();
    }

    /// Steps this row again in `tick`, or in the next tick if `tick` has passed; the soonest asked
    /// for holds.
    pub fn wake_at(&mut self, tick: Tick) {
        self.wake = self.wake.min(tick);
    }

    /// Roots or frees the body of the player this row is: a rooted body may turn, and fall, but
    /// not move over the ground.
    pub fn root(&mut self, rooted: bool) {
        self.order.get_or_insert_default().root = Some(rooted);
    }

    /// Puts the body of the player this row is at `at`, as if it had always stood there.
    pub fn place(&mut self, at: Spot) {
        self.order.get_or_insert_default().place = Some(at);
    }

    /// Plays `anim` once on the body of the player this row is, for the player and everyone who
    /// sees the body.
    pub fn play(&mut self, anim: Anim) {
        self.show(Show::Play(anim));
    }

    /// Tells the player this row is, and everyone who sees its body, that it attacked `target`,
    /// which came out as `outcome`: beside the swing it plays, it is what the client sounds. A
    /// target that is not a player's row has no body, and is told as none.
    pub fn attack(&mut self, target: Option<Id>, outcome: Outcome) {
        let target = target.filter(|t| t.is_player()).map(|t| t.n);
        self.show(Show::Attack(target, outcome));
    }

    /// Holds the body of the player this row is in `pose` until it is held in another, or let go
    /// with `None`; whoever comes to see the body is shown the pose it holds.
    pub fn hold(&mut self, pose: Option<Anim>) {
        self.show(Show::Hold(pose));
    }

    /// Idles the body of the player this row is in `anim` where it would stand, until it is idled
    /// in another or stops with `None`: it still moves as it would, and a held pose wins. Whoever
    /// comes to see the body is shown what it idles in.
    pub fn idle(&mut self, anim: Option<Anim>) {
        self.show(Show::Idle(anim));
    }

    fn show(&mut self, show: Show) {
        if self.me.is_player() {
            self.shows.push(BodyShow {
                body: self.me.n,
                phase: self.phase,
                show,
            });
        }
    }

    /// Adds `by` to the count of `what`, which a scenario's verdict reports.
    pub fn count(&mut self, what: &'static str, by: i64) {
        self.counts.push((what, by));
    }
}
