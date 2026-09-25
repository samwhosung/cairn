//! Melee, a game on cairn: every player fights every other hand to hand, and the dead rise at their spawn.

use game::{Game, Id, Kind, Letter, Out, Outcome, Tick, World, anim};

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
        pub ready_ms: u32,
    }
}

pub struct Melee;

game::saved! {
    pub struct Score {
        pub kills: u32,
        pub deaths: u32,
        pub dead: bool,
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
    DeadUnlaid { rises_at: Option<Tick> },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fighter {
    pub health: u32,
    pub life: Life,
    pub kills: u32,
    pub deaths: u32,
    pub swing_pending: bool,
    pub swings_again_at: Tick,
    pub stands_ready_until: Option<Tick>,
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
        let (health, life) = if score.dead {
            (
                0,
                Life::DeadUnlaid {
                    rises_at: rises_at(w),
                },
            )
        } else {
            (w.knobs().health, Life::Alive)
        };
        Fighter {
            health,
            life,
            kills: score.kills,
            deaths: score.deaths,
            swing_pending: false,
            swings_again_at: 0,
            stands_ready_until: None,
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
            dead: self.life != Life::Alive,
        }
    }

    fn step(id: Id, me: &mut Self, w: &World<'_, Melee>, out: &mut Out<Melee>) {
        match me.life {
            Life::DeadUnlaid { rises_at } => {
                me.life = Life::Dead { rises_at };
                lie_down(out);
                out.count("down", 1);
                rise(id, me, rises_at, w, out);
            }
            Life::Dead { rises_at } => rise(id, me, rises_at, w, out),
            Life::Alive => {
                if me.swing_pending {
                    swing(id, me, w, out);
                }
                calm_down_once_quiet(me, w, out);
            }
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
                    out.wake_at(me.swings_again_at.max(w.tick()));
                }
                Msg::Hit(damage) if me.life == Life::Alive => {
                    me.health = me.health.saturating_sub(damage);
                    if me.health == 0 {
                        die(me, letter.from, w, out);
                    } else {
                        out.play(anim::COMBAT_WOUND);
                        stand_ready(me, w, out);
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
    if w.tick() < me.swings_again_at {
        out.wake_at(me.swings_again_at);
        return;
    }
    let k = w.knobs();
    me.swing_pending = false;
    me.swings_again_at = w.after_ms(k.swing_ms);
    out.play(anim::ATTACK_UNARMED);
    stand_ready(me, w, out);
    out.count("swings", 1);
    if let Some(target) = nearest_in_front(id, w) {
        let damage = w.range(id, DAMAGE_ROLL, k.damage_min, k.damage_max) * k.damage_scale;
        out.send(target, Msg::Hit(damage));
        out.attack(Some(target), Outcome::Hit);
        out.count("hits", 1);
    } else {
        out.attack(None, Outcome::Miss);
    }
}

fn stand_ready(me: &mut Fighter, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    if me.stands_ready_until.is_none() {
        out.idle(Some(anim::READY_UNARMED));
    }
    let until = w.after_ms(w.knobs().ready_ms);
    me.stands_ready_until = Some(until);
    out.wake_at(until);
}

fn calm_down_once_quiet(me: &mut Fighter, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    match me.stands_ready_until {
        Some(at) if w.tick() >= at => {
            me.stands_ready_until = None;
            out.idle(None);
        }
        Some(at) => out.wake_at(at),
        None => {}
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

fn rises_at(w: &World<'_, Melee>) -> Option<Tick> {
    let respawn_s = w.knobs().respawn_s?;
    Some(w.after_ms(respawn_s.saturating_mul(1000)))
}

fn lie_down(out: &mut Out<Melee>) {
    out.root(true);
    out.hold(Some(anim::DEAD));
}

fn die(me: &mut Fighter, killer: Id, w: &World<'_, Melee>, out: &mut Out<Melee>) {
    let rises_at = rises_at(w);
    me.life = Life::Dead { rises_at };
    me.deaths += 1;
    me.swing_pending = false;
    if me.stands_ready_until.take().is_some() {
        out.idle(None);
    }
    if let Some(at) = rises_at {
        out.wake_at(at);
    }
    out.play(anim::DEATH);
    lie_down(out);
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
