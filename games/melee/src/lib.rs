//! Melee, a game on cairn: every player fights every other hand to hand, and the dead rise at their spawn.
//!
//! A swing hits the nearest living player within reach and in front, as last tick left them, for
//! a rolled damage. A player at no health is dead: rooted where it fell, it credits its killer,
//! and after some seconds, or never, it is placed back at its spawn at full health.

use game::{Game, Id, Kind, Letter, Out, Tick, World};

/// The action that swings.
pub const SWING: u32 = 1;

const DAMAGE_ROLL: u32 = 1;

game::knobs! {
    pub struct Knobs {
        pub health: u32,
        pub swing_ms: u32,
        /// How near a swing's target stands, yards.
        pub reach: f32,
        /// How wide an arc about the facing is in front, degrees.
        pub arc_deg: f32,
        pub damage_min: u32,
        pub damage_max: u32,
        pub damage_scale: u32,
        /// How long the dead wait to rise, or `never`.
        pub respawn_s: Option<u32>,
    }
}

pub struct Melee;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Msg {
    Swing,
    Hit(u32),
    Killed,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fighter {
    pub health: u32,
    pub dead: bool,
    pub kills: u32,
    pub deaths: u32,
    /// A swing asked for and not yet made.
    pub swinging: bool,
    pub ready_at: Tick,
    /// When the dead rise; `None` for the living, and for the dead for good.
    pub rises_at: Option<Tick>,
}

impl Game for Melee {
    const NAME: &'static str = "melee";
    const KNOBS: &'static str = include_str!("../knobs/base.knobs");
    const COUNTS: &'static [&'static str] =
        &["swings", "hits", "kills", "deaths", "respawns", "down"];
    type Knobs = Knobs;
    type Msg = Msg;
    type Player = Fighter;

    fn join(_: Id, w: &World<'_, Self>) -> Fighter {
        Fighter {
            health: w.knobs().health,
            dead: false,
            kills: 0,
            deaths: 0,
            swinging: false,
            ready_at: 0,
            rises_at: None,
        }
    }

    fn action(number: u32) -> Option<Msg> {
        (number == SWING).then_some(Msg::Swing)
    }
}

impl Kind<Melee> for Fighter {
    type Sent = (u32, bool);
    type Saved = (u32, u32);

    fn sent(&self) -> (u32, bool) {
        (self.health, self.dead)
    }

    fn saved(&self) -> (u32, u32) {
        (self.kills, self.deaths)
    }

    fn step(id: Id, me: &mut Self, w: &World<'_, Melee>, out: &mut Out<Melee>) {
        if me.dead {
            rise(id, me, w, out);
        } else if me.swinging {
            swing(id, me, w, out);
        }
    }

    fn apply(
        id: Id,
        me: &mut Self,
        mail: &[Letter<Msg>],
        w: &World<'_, Melee>,
        out: &mut Out<Melee>,
    ) {
        let _ = id;
        for letter in mail {
            match letter.msg {
                Msg::Swing if !me.dead => {
                    me.swinging = true;
                    out.wake_at(me.ready_at.max(w.tick()));
                }
                Msg::Hit(damage) if !me.dead => {
                    me.health = me.health.saturating_sub(damage);
                    if me.health == 0 {
                        die(me, letter.from, w, out);
                    }
                }
                Msg::Killed => {
                    me.kills += 1;
                    out.count("kills", 1);
                }
                Msg::Swing | Msg::Hit(_) => {}
            }
        }
    }
}

fn swing(id: Id, me: &mut Fighter, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    if w.tick() < me.ready_at {
        out.wake_at(me.ready_at);
        return;
    }
    let k = w.knobs();
    me.swinging = false;
    me.ready_at = w.after_ms(k.swing_ms);
    out.count("swings", 1);
    if let Some(target) = target(id, w) {
        let damage = w.range(id, DAMAGE_ROLL, k.damage_min, k.damage_max) * k.damage_scale;
        out.send(target, Msg::Hit(damage));
        out.count("hits", 1);
    }
}

/// The nearest living player within reach and in front, the lowest numbered of a tie.
fn target(id: Id, w: &World<'_, Melee>) -> Option<Id> {
    let me = w.body(id)?;
    let k = w.knobs();
    let arc = k.arc_deg.to_radians();
    let mut best: Option<(f32, Id)> = None;
    w.near(me.pos, k.reach, |other, at| {
        let living = w.player(other).is_some_and(|f| !f.dead);
        if other == id || !living || !me.faces(at.pos, arc) {
            return;
        }
        let d = me.across(at.pos);
        if best.is_none_or(|(bd, bid)| d.total_cmp(&bd).then(other.cmp(&bid)).is_lt()) {
            best = Some((d, other));
        }
    });
    best.map(|(_, id)| id)
}

fn die(me: &mut Fighter, killer: Id, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    me.dead = true;
    me.deaths += 1;
    me.swinging = false;
    me.rises_at = w
        .knobs()
        .respawn_s
        .map(|s| w.after_ms(s.saturating_mul(1000)));
    if let Some(at) = me.rises_at {
        out.wake_at(at);
    }
    out.root(true);
    out.send(killer, Msg::Killed);
    out.count("deaths", 1);
    out.count("down", 1);
}

fn rise(id: Id, me: &mut Fighter, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    let Some(at) = me.rises_at else { return };
    if w.tick() < at {
        out.wake_at(at);
        return;
    }
    let Some(spawn) = w.spawn(id) else { return };
    me.dead = false;
    me.health = w.knobs().health;
    me.rises_at = None;
    out.place(spawn);
    out.root(false);
    out.count("respawns", 1);
    out.count("down", -1);
}
