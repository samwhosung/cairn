use std::fmt;

use game::{Hosted, Id, Shows};
use protocol::{
    SLOTS, Show, Whose, Wrapped, begin_batch, finish_frame, write_appear, write_correct,
    write_game, write_granted, write_move, write_place, write_show, write_state, write_turn,
    write_vanish,
};

use crate::grid::Grid;
use crate::limits::Limits;
use crate::net::{Outbox, Shared};
use crate::relays::{Hot, Relays};
use crate::world::World;

const HELD_CLAIM_AGE_S: f32 = 2.0;

#[derive(Clone, Copy, Debug)]
struct Reach {
    across: f32,
    up: f32,
}

impl Reach {
    fn of(limits: &Limits) -> Self {
        let fastest = [
            limits.walk,
            limits.run,
            limits.run_back,
            limits.swim,
            limits.swim_back,
        ]
        .into_iter()
        .fold(0.0, f32::max);
        Self {
            across: (Wrapped::REACH_YD[0] - fastest * HELD_CLAIM_AGE_S).max(0.0),
            up: (Wrapped::REACH_YD[2] - limits.fall * HELD_CLAIM_AGE_S).max(0.0),
        }
    }
}

/// How far a player sees, and how often what it sees is refreshed. An entity further from where
/// the server holds the player than its client reads a batch's positions right does not come into
/// view, and one in view leaves once a change of it would be sent.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Entities within this many yards come into view.
    pub radius: f32,
    /// Yards past the radius an entity in view must go before it leaves.
    pub grey: f32,
    /// Ticks between rechecks of who is in view, staggered across observers.
    pub aoi_every: u32,
    /// Movement refreshes by distance, nearest tier first. A change of how an entity moves goes
    /// out at once at any distance.
    pub tiers: [Tier; 3],
    /// A client with more than this many bytes queued gets no refreshes until it drains.
    pub shed_bytes: usize,
    /// A client that says it is more than this many ticks behind gets no refreshes until it
    /// catches up.
    pub shed_ticks: u32,
    /// A client with more than this many bytes queued is dropped: its connection closes and its
    /// player leaves at the next tick.
    pub kick_bytes: usize,
    /// A client that says it is more than this many ticks behind is dropped as for
    /// [`View::kick_bytes`].
    pub kick_ticks: u32,
}

/// Within `within` yards, at most one movement refresh per `every` ticks.
#[derive(Clone, Copy, Debug)]
pub struct Tier {
    pub within: f32,
    pub every: u32,
}

impl Default for View {
    fn default() -> Self {
        Self {
            radius: 100.0,
            grey: 1.0,
            aoi_every: 5,
            tiers: [
                Tier {
                    within: 25.0,
                    every: 1,
                },
                Tier {
                    within: 50.0,
                    every: 4,
                },
                Tier {
                    within: f32::INFINITY,
                    every: 10,
                },
            ],
            shed_bytes: 256 << 10,
            shed_ticks: 10,
            kick_bytes: 2 << 20,
            kick_ticks: 200,
        }
    }
}

/// A view that reaches further than a batch's positions read right, so it would hide players in
/// range, in yards across: the view's radius and grey, and the reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PastReach {
    pub view_yd: f32,
    pub reach_yd: f32,
}

impl fmt::Display for PastReach {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a view of {:.1} yd, its radius and grey, goes past the {:.1} yd a batch's positions \
             reach with these movement limits",
            self.view_yd, self.reach_yd
        )
    }
}

impl std::error::Error for PastReach {}

impl View {
    /// Refuses a view whose radius and grey go past what a batch's positions reach under `limits`.
    pub fn check(&self, limits: &Limits) -> Result<(), PastReach> {
        let (view_yd, reach_yd) = (self.radius + self.grey, Reach::of(limits).across);
        if view_yd <= reach_yd {
            Ok(())
        } else {
            Err(PastReach { view_yd, reach_yd })
        }
    }

    fn tier(&self, d2: f32) -> usize {
        self.tiers
            .iter()
            .position(|t| d2 <= t.within * t.within)
            .unwrap_or(self.tiers.len() - 1)
    }
}

#[derive(Clone, Copy, Debug)]
struct Seen {
    id: u32,
    sent_tick: u32,
    slot: u16,
}

#[derive(Default)]
struct Slots {
    freed: Vec<u16>,
    first_unused: u16,
}

impl Slots {
    fn take(&mut self) -> Option<u16> {
        if let Some(slot) = self.freed.pop() {
            return Some(slot);
        }
        let slot = self.first_unused;
        (slot < SLOTS).then(|| {
            self.first_unused += 1;
            slot
        })
    }
}

pub struct Observer {
    pub id: u32,
    conn: u32,
    seen: Vec<Seen>,
    slots: Slots,
    fresh: bool,
    size_hint: usize,
    pub outbox: Option<Outbox>,
}

impl Observer {
    pub fn new(id: u32, conn: u32, outbox: Option<Outbox>) -> Self {
        Self {
            id,
            conn,
            seen: Vec::new(),
            slots: Slots::default(),
            fresh: true,
            size_hint: 64,
            outbox,
        }
    }

    pub fn in_view(&self) -> Vec<(u16, u32)> {
        self.seen.iter().map(|e| (e.slot, e.id)).collect()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Built {
    pub appeared: u32,
    pub vanished: u32,
    pub moves: u32,
    pub turns: u32,
    pub states: u32,
    pub moves_and_turns_by_tier: [u32; 3],
    pub appears_without_slot: u32,
    pub deferred: u32,
    pub corrections: u32,
    pub kicked: u32,
    pub bytes: u64,
    pub movement_bytes: u64,
    pub shared_bytes: u64,
}

impl Built {
    pub fn add(self, o: Self) -> Self {
        Self {
            appeared: self.appeared + o.appeared,
            vanished: self.vanished + o.vanished,
            moves: self.moves + o.moves,
            turns: self.turns + o.turns,
            states: self.states + o.states,
            moves_and_turns_by_tier: std::array::from_fn(|t| {
                self.moves_and_turns_by_tier[t] + o.moves_and_turns_by_tier[t]
            }),
            appears_without_slot: self.appears_without_slot + o.appears_without_slot,
            deferred: self.deferred + o.deferred,
            corrections: self.corrections + o.corrections,
            kicked: self.kicked + o.kicked,
            bytes: self.bytes + o.bytes,
            movement_bytes: self.movement_bytes + o.movement_bytes,
            shared_bytes: self.shared_bytes + o.shared_bytes,
        }
    }
}

#[derive(Default)]
pub struct Scratch {
    near: Vec<u32>,
    kept: Vec<Seen>,
    came: Vec<u32>,
    seen: Vec<Seen>,
    near_at: Vec<u32>,
    kept_at: Vec<u32>,
    recheck: u32,
}

pub struct Scene<'a> {
    pub world: &'a World,
    pub grid: &'a Grid,
    pub view: &'a View,
    pub relays: &'a Relays,
    pub clients: &'a Shared,
    pub game: Option<&'a dyn Hosted>,
}

pub fn send_batch(o: &mut Observer, scene: &Scene<'_>, s: &mut Scratch) -> Built {
    let mut built = Built::default();
    let world = scene.world;
    let tick = world.tick();
    let view = scene.view;
    let queued = o.outbox.as_ref().map_or(0, Outbox::queued_bytes);
    let behind = o
        .outbox
        .as_ref()
        .and_then(Outbox::behind_ticks)
        .unwrap_or(0);
    if queued > view.kick_bytes || behind > view.kick_ticks {
        o.outbox = None;
        scene.clients.leave_next_tick(o.conn);
        built.kicked = 1;
    }
    let bodies = world.stepped();
    let me = bodies[o.id as usize];
    let mut out = Vec::with_capacity(o.size_hint);
    let start = begin_batch(&mut out, tick);
    if let Some(p) = me.placed.filter(|p| p.tick == tick) {
        write_place(&mut out, me.correction_seq, p.rooted, &me.movement);
    } else {
        if let Some(c) = me.corrected
            && c.tick == tick
        {
            write_correct(&mut out, me.correction_seq, c.why, &me.movement);
            built.corrections += 1;
        }
        if me.teleported_at == Some(tick) {
            write_granted(&mut out, &me.movement);
        }
    }
    let mut pass = Pass {
        me: me.movement.pos,
        reach: Reach::of(world.limits()),
        relays: scene.relays,
        game: scene.game,
        view,
        tick,
        shedding: queued > view.shed_bytes || behind > view.shed_ticks,
        slots: &mut o.slots,
        out: &mut out,
        built: &mut built,
    };
    let rechecked = o.fresh || (tick + o.id).is_multiple_of(view.aoi_every.max(1));
    if rechecked {
        s.near.clear();
        scene.grid.present_within(
            bodies,
            me.movement.pos,
            view.radius + view.grey,
            &mut s.near,
        );
        s.near.retain(|&n| n != o.id);
        pass.recheck(&o.seen, s);
        std::mem::swap(&mut o.seen, &mut s.seen);
        o.fresh = false;
    } else {
        o.seen.retain_mut(|e| pass.keep(e));
    }
    if let Some(game) = scene.game {
        let came = if rechecked { &s.came[..] } else { &[] };
        pass.write_game_changes(&o.seen, &game.record().shown, came);
        pass.write_shows(o.id, &o.seen, game.shows(), came);
    }
    finish_frame(&mut out, start);
    built.bytes = out.len() as u64;
    o.size_hint = out.len();
    if let Some(outbox) = &o.outbox {
        outbox.send(out);
    }
    built
}

struct Pass<'a> {
    me: [f32; 3],
    reach: Reach,
    relays: &'a Relays,
    game: Option<&'a dyn Hosted>,
    view: &'a View,
    tick: u32,
    shedding: bool,
    slots: &'a mut Slots,
    out: &'a mut Vec<u8>,
    built: &'a mut Built,
}

impl Pass<'_> {
    fn across2(&self, h: &Hot) -> f32 {
        (self.me[0] - h.pos[0]).powi(2) + (self.me[1] - h.pos[1]).powi(2)
    }

    fn within_reach(&self, h: &Hot, across2: f32) -> bool {
        across2 <= self.reach.across * self.reach.across
            && (self.me[2] - h.pos[2]).abs() <= self.reach.up
    }

    fn recheck(&mut self, seen: &[Seen], s: &mut Scratch) {
        debug_assert!(seen.is_sorted_by_key(|e| e.id));
        let entities = self.relays.hot.len();
        s.near_at.resize(entities, 0);
        s.kept_at.resize(entities, 0);
        s.recheck += 1;
        let r = s.recheck;
        for &n in &s.near {
            s.near_at[n as usize] = r;
        }
        s.kept.clear();
        for &e in seen {
            if s.near_at[e.id as usize] == r {
                s.kept_at[e.id as usize] = r;
                let mut e = e;
                if self.refresh_or_let_go(&mut e) {
                    s.kept.push(e);
                }
            } else {
                self.vanish(e.slot);
            }
        }
        let r2 = self.view.radius * self.view.radius;
        s.came.clear();
        s.came.extend(s.near.iter().copied().filter(|&n| {
            if s.kept_at[n as usize] == r {
                return false;
            }
            let h = &self.relays.hot[n as usize];
            let across2 = self.across2(h);
            across2 <= r2 && self.within_reach(h, across2)
        }));
        s.came.sort_unstable();
        s.seen.clear();
        let mut kept = s.kept.iter().copied().peekable();
        for &n in &s.came {
            let Some(slot) = self.slots.take() else {
                self.built.appears_without_slot += 1;
                continue;
            };
            self.appear(n, slot);
            while let Some(e) = kept.next_if(|e| e.id < n) {
                s.seen.push(e);
            }
            s.seen.push(Seen {
                id: n,
                sent_tick: self.tick,
                slot,
            });
        }
        s.seen.extend(kept);
    }

    fn appear(&mut self, id: u32, slot: u16) {
        let (intro, relay) = (
            &self.relays.intros[id as usize],
            &self.relays.pieces[id as usize],
        );
        self.built.shared_bytes += write_appear(self.out, slot, intro, relay) as u64;
        self.built.appeared += 1;
        let Some(game) = self.game else { return };
        if let Some(state) = game.shown(id) {
            self.built.shared_bytes += write_game(self.out, slot, state) as u64;
        }
        if let Some(pose) = game.held(id) {
            write_show(self.out, Whose::Slot(slot), Show::Hold(Some(pose.0)));
        }
    }

    /// All three are by id, and those that came were sent their whole state as they appeared.
    fn write_game_changes(&mut self, seen: &[Seen], shown: &[(Id, Vec<u8>)], came: &[u32]) {
        let mut rest = seen;
        for (id, state) in shown.iter().take_while(|(id, _)| id.is_player()) {
            rest = &rest[rest.partition_point(|e| e.id < id.n)..];
            let Some(e) = rest.first() else { break };
            if e.id == id.n && came.binary_search(&id.n).is_err() {
                self.built.shared_bytes += write_game(self.out, e.slot, state) as u64;
            }
        }
    }

    fn write_shows(&mut self, me: u32, seen: &[Seen], shows: &Shows, came: &[u32]) {
        let whose = |n: u32| {
            if n == me {
                return Some(Whose::Own);
            }
            let at = seen.binary_search_by_key(&n, |e| e.id).ok()?;
            Some(Whose::Slot(seen[at].slot))
        };
        for &(n, anim) in &shows.played {
            if let Some(whose) = whose(n) {
                write_show(self.out, whose, Show::Play(anim.0));
            }
        }
        for &(n, pose) in &shows.held {
            let posed_as_it_appeared = came.binary_search(&n).is_ok();
            if let Some(whose) = whose(n).filter(|_| !posed_as_it_appeared) {
                write_show(self.out, whose, Show::Hold(pose.map(|a| a.0)));
            }
        }
    }

    fn vanish(&mut self, slot: u16) {
        write_vanish(self.out, slot);
        self.slots.freed.push(slot);
        self.built.vanished += 1;
    }

    fn keep(&mut self, e: &mut Seen) -> bool {
        if !self.relays.hot[e.id as usize].present {
            self.vanish(e.slot);
            return false;
        }
        self.refresh_or_let_go(e)
    }

    fn refresh_or_let_go(&mut self, e: &mut Seen) -> bool {
        let h = &self.relays.hot[e.id as usize];
        if h.state_changed_at
            .max(h.pos_changed_at)
            .max(h.facing_changed_at)
            <= e.sent_tick
        {
            return true;
        }
        let across2 = self.across2(h);
        if !self.within_reach(h, across2) {
            self.vanish(e.slot);
            return false;
        }
        let relay = &self.relays.pieces[e.id as usize];
        let shared = if h.state_changed_at > e.sent_tick {
            self.built.states += 1;
            write_state(self.out, e.slot, relay)
        } else {
            if self.shedding {
                self.built.deferred += 1;
                return true;
            }
            let tier = self.view.tier(across2);
            if self.tick - e.sent_tick < self.view.tiers[tier].every {
                return true;
            }
            self.built.moves_and_turns_by_tier[tier] += 1;
            if h.pos_changed_at > e.sent_tick {
                self.built.moves += 1;
                write_move(self.out, e.slot, relay)
            } else {
                self.built.turns += 1;
                write_turn(self.out, e.slot, relay)
            }
        };
        self.built.shared_bytes += shared as u64;
        self.built.movement_bytes += 2 + shared as u64;
        e.sent_tick = self.tick;
        true
    }
}
