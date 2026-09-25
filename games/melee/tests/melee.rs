//! Melee played headless on the rule API, its bodies standing where they joined.

use std::f32::consts::PI;
use std::ops::Range;

use game::{BodyOrder, Delivery, Engine, Hosted, Id, Knobs as _, Spot, Turn};
use melee::{Fighter, Knobs, Melee, SWING};

const QUICK: &str = "swing_ms = 50\ndamage_min = 30\ndamage_max = 30\nrespawn_s = 1\n";

/// Melee on its own knobs with `overlays` laid on them in turn.
fn engine(overlays: &[&str]) -> Engine<Melee> {
    let base = game::lines(include_str!("../knobs/base.knobs"), "base.knobs").expect("lines");
    let over: Vec<game::Line> = overlays
        .iter()
        .flat_map(|o| game::lines(o, "overlay").expect("lines"))
        .collect();
    let knobs = Knobs::read(&base, "base.knobs", &over).expect("knobs");
    Engine::new(knobs, 3, 50, Delivery::Canonical)
}

fn at(x: f32, y: f32, facing: f32) -> Spot {
    Spot {
        pos: [x, y, 0.0],
        facing,
    }
}

/// Runs `ticks`, player 0 swinging in every one, and returns each tick's orders for the bodies.
fn fight(e: &mut Engine<Melee>, bodies: &[Spot], ticks: Range<u32>) -> Vec<(u32, u32, BodyOrder)> {
    let joined: Vec<(u32, Spot)> = (0..).zip(bodies.iter().copied()).collect();
    let present: Vec<Option<Spot>> = bodies.iter().copied().map(Some).collect();
    let mut orders = Vec::new();
    for tick in ticks {
        e.tick(&Turn {
            tick,
            joined: if tick == 0 { &joined[..] } else { &[] },
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
    assert!(!dead.dead && dead.health == 100, "{dead:?}");
    let counts = e.counts();
    assert_eq!(
        (counts["deaths"], counts["respawns"], counts["down"]),
        (1, 1, 0)
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
    assert!(fighter(&e, 1).dead);
    assert_eq!((e.counts()["respawns"], e.counts()["down"]), (0, 1));
    let mut e = engine(&[QUICK, include_str!("../knobs/dangerous.knobs")]);
    fight(&mut e, &bodies, 0..1);
    assert_eq!(fighter(&e, 1).health, 40);
}

#[test]
fn its_own_knobs_and_its_overlays_load() {
    for overlay in [
        "",
        include_str!("../knobs/permadeath.knobs"),
        include_str!("../knobs/dangerous.knobs"),
    ] {
        let over = game::lines(overlay, "overlay").expect("lines");
        game::load::<Melee>(None, &over, 1).expect("loads");
    }
}
