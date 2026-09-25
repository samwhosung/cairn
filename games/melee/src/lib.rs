//! Melee, a game on cairn: every player fights every other hand to hand, and the dead rise at their spawn.

use game::{Game, Id, Kind, Letter, Out, Tick, World, anim};

pub const SWING: u32 = 1;

const DAMAGE_ROLL: u32 = 1;

game::knobs! {
    pub struct Knobs {
        pub health: u32,
        pub swing_ms: u32,
        pub reach_yd: f32,
        pub arc_deg: f32,
        pub damage_min: u32,
        pub damage_max: u32,
        pub damage_scale: u32,
        pub respawn_s: Option<u32>,
    }
}

pub struct Melee;

game::saved! {
    /// What a fighter keeps from one visit to the next.
    pub struct Score {
        pub kills: u32,
        pub deaths: u32,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Msg {
    Swing,
    Hit(u32),
    Killed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Life {
    Alive,
    Dead { rises_at: Option<Tick> },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fighter {
    pub health: u32,
    pub life: Life,
    pub kills: u32,
    pub deaths: u32,
    pub swing_pending: bool,
    pub ready_at: Tick,
}

impl Game for Melee {
    const NAME: &'static str = "melee";
    const KNOBS: &'static str = include_str!("../knobs/base.knobs");
    const COUNTS: &'static [&'static str] =
        &["swings", "hits", "kills", "deaths", "respawns", "down"];
    type Knobs = Knobs;
    type Msg = Msg;
    type Player = Fighter;

    fn join(_: Id, saved: Option<Score>, w: &World<'_, Self>) -> Fighter {
        let score = saved.unwrap_or_default();
        Fighter {
            health: w.knobs().health,
            life: Life::Alive,
            kills: score.kills,
            deaths: score.deaths,
            swing_pending: false,
            ready_at: 0,
        }
    }

    fn action(number: u32) -> Option<Msg> {
        (number == SWING).then_some(Msg::Swing)
    }
}

impl Kind<Melee> for Fighter {
    type Sent = (u32, bool);
    type Saved = Score;

    fn sent(&self) -> (u32, bool) {
        (self.health, self.life != Life::Alive)
    }

    fn saved(&self) -> Score {
        Score {
            kills: self.kills,
            deaths: self.deaths,
        }
    }

    fn step(id: Id, me: &mut Self, w: &World<'_, Melee>, out: &mut Out<Melee>) {
        match me.life {
            Life::Dead { rises_at } => rise(id, me, rises_at, w, out),
            Life::Alive if me.swing_pending => swing(id, me, w, out),
            Life::Alive => {}
        }
    }

    fn apply(
        _: Id,
        me: &mut Self,
        mail: &[Letter<Msg>],
        w: &World<'_, Melee>,
        out: &mut Out<Melee>,
    ) {
        for letter in mail {
            match letter.msg {
                Msg::Swing if me.life == Life::Alive => {
                    me.swing_pending = true;
                    out.wake_at(me.ready_at.max(w.tick()));
                }
                Msg::Hit(damage) if me.life == Life::Alive => {
                    me.health = me.health.saturating_sub(damage);
                    if me.health == 0 {
                        die(me, letter.from, w, out);
                    } else {
                        out.play(anim::COMBAT_WOUND);
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
    me.swing_pending = false;
    me.ready_at = w.after_ms(k.swing_ms);
    out.play(anim::ATTACK_UNARMED);
    out.count("swings", 1);
    if let Some(target) = nearest_in_front(id, w) {
        let damage = w.range(id, DAMAGE_ROLL, k.damage_min, k.damage_max) * k.damage_scale;
        out.send(target, Msg::Hit(damage));
        out.count("hits", 1);
    }
}

fn nearest_in_front(id: Id, w: &World<'_, Melee>) -> Option<Id> {
    let me = w.body(id)?;
    let k = w.knobs();
    let arc = k.arc_deg.to_radians();
    let mut best: Option<(f32, Id)> = None;
    w.near(me.pos, k.reach_yd, |other, at| {
        let alive = w.player(other).is_some_and(|f| f.life == Life::Alive);
        if other == id || !alive || !me.faces(at.pos, arc) {
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
    let rises_at = w
        .knobs()
        .respawn_s
        .map(|s| w.after_ms(s.saturating_mul(1000)));
    me.life = Life::Dead { rises_at };
    me.deaths += 1;
    me.swing_pending = false;
    if let Some(at) = rises_at {
        out.wake_at(at);
    }
    out.root(true);
    out.play(anim::DEATH);
    out.hold(Some(anim::DEAD));
    out.send(killer, Msg::Killed);
    out.count("deaths", 1);
    out.count("down", 1);
}

fn rise(
    id: Id,
    me: &mut Fighter,
    rises_at: Option<Tick>,
    w: &World<'_, Melee>,
    out: &mut Out<Melee>,
) {
    let Some(at) = rises_at else { return };
    if w.tick() < at {
        out.wake_at(at);
        return;
    }
    let Some(spawn) = w.spawn(id) else { return };
    me.life = Life::Alive;
    me.health = w.knobs().health;
    out.place(spawn);
    out.root(false);
    out.hold(None);
    out.count("respawns", 1);
    out.count("down", -1);
}
