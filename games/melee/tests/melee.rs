use std::f32::consts::PI;
use std::ops::Range;

use game::{BodyOrder, Bytes, Delivery, Engine, Hosted, Id, Knobs as _, Shows, Spot, Turn, anim};
use melee::{Fighter, Knobs, Life, Melee, SWING, Score};

const QUICK: &str = "swing_ms = 50\ndamage_min = 30\ndamage_max = 30\nrespawn_s = 1\n";

fn engine(overlays: &[&str]) -> Engine<Melee> {
    let own = include_str!("../knobs/base.knobs");
    let base = game::KnobsFile::parse(own, "base.knobs").expect("its own knobs");
    let over: Vec<game::Line> = overlays
        .iter()
        .flat_map(|o| game::KnobsFile::parse(o, "overlay").expect("lines").lines)
        .collect();
    let knobs = Knobs::read(&base, &over).expect("knobs");
    Engine::new(knobs, 3, 50, Delivery::Canonical)
}

fn at(x: f32, y: f32, facing: f32) -> Spot {
    Spot {
        pos: [x, y, 0.0],
        facing,
    }
}

fn fight(e: &mut Engine<Melee>, bodies: &[Spot], ticks: Range<u32>) -> Vec<(u32, u32, BodyOrder)> {
    let joined: Vec<(u32, Spot)> = (0..).zip(bodies.iter().copied()).collect();
    let present: Vec<Option<Spot>> = bodies.iter().copied().map(Some).collect();
    let mut orders = Vec::new();
    for tick in ticks {
        e.tick(&Turn {
            tick,
            joined: if tick == 0 { &joined[..] } else { &[] },
            restored: &[],
            bodies: &present,
            actions: &[(0, SWING)],
            cpu_ns: || 0,
        });
        orders.extend(e.orders().iter().map(|&(n, o)| (tick, n, o)));
    }
    orders
}

fn fighter(e: &Engine<Melee>, n: u32) -> Fighter {
    e.world().player(Id::player(n)).expect("a fighter").clone()
}

#[test]
fn a_swing_kills_in_front_the_killer_is_credited_and_the_dead_rise_at_their_spawn() {
    let mut e = engine(&[QUICK]);
    let bodies = [at(0.0, 0.0, 0.0), at(3.0, 0.0, PI)];
    assert_eq!(fight(&mut e, &bodies, 0..3), []);
    assert_eq!(fighter(&e, 1).health, 10, "three hits of thirty");
    let orders = fight(&mut e, &bodies, 3..24);
    let rooted = BodyOrder {
        place: None,
        root: Some(true),
    };
    let risen = BodyOrder {
        place: Some(bodies[1]),
        root: Some(false),
    };
    assert_eq!(orders, [(3, 1, rooted), (23, 1, risen)]);
    let (killer, dead) = (fighter(&e, 0), fighter(&e, 1));
    assert_eq!((killer.kills, dead.deaths), (1, 1));
    assert!(dead.life == Life::Alive && dead.health == 100, "{dead:?}");
    let counts = e.counts();
    assert_eq!(
        (counts["deaths"], counts["respawns"], counts["down"]),
        (1, 1, 0)
    );
}

#[test]
fn a_swing_plays_the_attack_a_hit_the_wound_and_the_dead_lie_until_they_rise() {
    let mut e = engine(&[QUICK]);
    let bodies = [at(0.0, 0.0, 0.0), at(3.0, 0.0, PI)];
    let mut shown: Vec<Shows> = Vec::new();
    for tick in 0..24 {
        fight(&mut e, &bodies, tick..tick + 1);
        shown.push(e.shows().clone());
    }
    let wounded = Shows {
        played: vec![(0, anim::ATTACK_UNARMED), (1, anim::COMBAT_WOUND)],
        held: vec![],
    };
    assert_eq!(shown[..3], [wounded.clone(), wounded.clone(), wounded]);
    let killed = Shows {
        played: vec![(0, anim::ATTACK_UNARMED), (1, anim::DEATH)],
        held: vec![(1, Some(anim::DEAD))],
    };
    assert_eq!(
        shown[3], killed,
        "the killing blow plays Death, not a wound"
    );
    let missed = Shows {
        played: vec![(0, anim::ATTACK_UNARMED)],
        held: vec![],
    };
    assert!(
        shown[4..23].iter().all(|s| *s == missed),
        "a swing at no one"
    );
    assert_eq!(e.held(1), None);
    assert_eq!(
        shown[23].held,
        [(1, None)],
        "the dead rise and let go of the pose"
    );
}

#[test]
fn a_swing_takes_the_nearest_in_front_and_the_lowest_numbered_of_a_tie() {
    let mut e = engine(&[QUICK]);
    let bodies = [
        at(0.0, 0.0, 0.0),
        at(-2.0, 0.0, 0.0),
        at(2.0, 2.0, 0.0),
        at(2.0, -2.0, 0.0),
    ];
    fight(&mut e, &bodies, 0..1);
    let health: Vec<u32> = (1..4).map(|n| fighter(&e, n).health).collect();
    assert_eq!(
        health,
        [100, 70, 100],
        "the one behind is out, and 2 wins the tie"
    );
}

#[test]
fn permadeath_leaves_the_dead_down_and_a_dangerous_world_hits_twice_as_hard() {
    let bodies = [at(0.0, 0.0, 0.0), at(3.0, 0.0, PI)];
    let mut e = engine(&[QUICK, include_str!("../knobs/permadeath.knobs")]);
    fight(&mut e, &bodies, 0..60);
    assert_eq!(fighter(&e, 1).life, Life::Dead { rises_at: None });
    assert_eq!((e.counts()["respawns"], e.counts()["down"]), (0, 1));
    let mut e = engine(&[QUICK, include_str!("../knobs/dangerous.knobs")]);
    fight(&mut e, &bodies, 0..1);
    assert_eq!(fighter(&e, 1).health, 40);
}

#[test]
fn a_fighter_that_comes_back_keeps_its_kills_and_deaths_and_rises_whole() {
    let mut e = engine(&[]);
    let score = Score {
        kills: 4,
        deaths: 9,
        dead: false,
    };
    let spot = at(0.0, 0.0, 0.0);
    e.tick(&Turn {
        tick: 0,
        joined: &[(0, spot), (1, spot)],
        restored: &[(1, score.to_bytes())],
        bodies: &[Some(spot), Some(spot)],
        actions: &[],
        cpu_ns: || 0,
    });
    let (fresh, back) = (fighter(&e, 0), fighter(&e, 1));
    assert_eq!((fresh.kills, fresh.deaths), (0, 0));
    assert_eq!((back.kills, back.deaths, back.health), (4, 9, 100));
    let saved: Vec<_> = e
        .record()
        .saved
        .iter()
        .map(|(id, s)| (id.n, s.clone()))
        .collect();
    assert_eq!(saved[1], (1, Some(score.to_bytes())));
}

/// A fighter that saved itself dead joins beside a live one, and the engine runs `ticks` more.
fn comes_back_dead(overlays: &[&str], ticks: u32) -> (Engine<Melee>, Vec<(u32, u32, BodyOrder)>) {
    let mut e = engine(overlays);
    let dead = Score {
        kills: 2,
        deaths: 3,
        dead: true,
    };
    let (spot, far) = (at(0.0, 0.0, 0.0), at(40.0, 0.0, 0.0));
    let restored = [(1, dead.to_bytes())];
    let mut orders = Vec::new();
    for tick in 0..=ticks {
        let joined = [(0, spot), (1, far)];
        e.tick(&Turn {
            tick,
            joined: if tick == 0 { &joined[..] } else { &[] },
            restored: if tick == 0 { &restored[..] } else { &[] },
            bodies: &[Some(spot), Some(far)],
            actions: &[],
            cpu_ns: || 0,
        });
        orders.extend(e.orders().iter().map(|&(n, o)| (tick, n, o)));
    }
    (e, orders)
}

#[test]
fn a_fighter_that_comes_back_dead_lies_down_at_once_and_rises_on_the_usual_timer() {
    let (e, orders) = comes_back_dead(&[], 0);
    let back = fighter(&e, 1);
    assert_eq!((back.health, back.kills, back.deaths), (0, 2, 3));
    assert_eq!(
        back.life,
        Life::Dead {
            rises_at: Some(200)
        },
        "ten seconds after it came back"
    );
    let laid = BodyOrder {
        place: None,
        root: Some(true),
    };
    assert_eq!(orders, [(0, 1, laid)]);
    assert_eq!((e.held(1), e.held(0)), (Some(anim::DEAD), None));
    assert_eq!(e.counts()["down"], 1);
    let saved = |e: &Engine<Melee>| {
        let bytes = &e.saved()[&Id::player(1)];
        Score::from_bytes(bytes).expect("a score")
    };
    assert!(saved(&e).dead);
    let (e, orders) = comes_back_dead(&[], 200);
    let risen = BodyOrder {
        place: Some(at(40.0, 0.0, 0.0)),
        root: Some(false),
    };
    assert_eq!(orders.last(), Some(&(200, 1, risen)));
    assert_eq!((fighter(&e, 1).health, e.held(1)), (100, None));
    assert!(!saved(&e).dead);
    assert_eq!(e.counts()["down"], 0);
}

#[test]
fn under_permadeath_a_fighter_that_comes_back_dead_stays_down() {
    let permadeath = include_str!("../knobs/permadeath.knobs");
    let (e, orders) = comes_back_dead(&[permadeath], 400);
    assert_eq!(fighter(&e, 1).life, Life::Dead { rises_at: None });
    assert_eq!(orders.len(), 1, "laid down once, never raised: {orders:?}");
    assert_eq!(e.held(1), Some(anim::DEAD));
}

#[test]
fn its_own_knobs_and_its_overlays_load() {
    for overlay in [
        "",
        include_str!("../knobs/permadeath.knobs"),
        include_str!("../knobs/dangerous.knobs"),
    ] {
        let over = game::KnobsFile::parse(overlay, "overlay").expect("lines");
        game::load::<Melee>(None, &over.lines, 1).expect("loads");
    }
}
