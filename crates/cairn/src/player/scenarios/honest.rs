//! The real mover walking the scenarios while it claims every step to a server that checks them:
//! the server never has cause to put an honest client back.

use bevy::input::keyboard::KeyCode;
use server::{Config, Running, Spawn, Why};
use world::unit::CharacterLook;

use super::walker::Walker;
use super::{
    ABBEY_STAIRS, CANAL_RAMP, DOWN_THE_RAMP, HILLSIDE, INN_WALL, MEADOW, SHORE, walk_path,
};

const HZ: f32 = 60.0;

/// A server on one thread of its own that sets its players at `spawns` in turn.
pub fn serve(spawns: &[([f32; 3], f32)]) -> Running {
    server::start(Config {
        tick_threads: 1,
        io_threads: 1,
        spawns: spawns
            .iter()
            .map(|&(pos, heading_deg)| Spawn {
                pos,
                facing: heading_deg.to_radians(),
            })
            .collect(),
        ..Config::default()
    })
    .expect("a server")
}

struct Scenario {
    name: &'static str,
    feet: [f32; 3],
    heading_deg: f32,
    walk: fn(&mut Walker),
}

fn hold_w(w: &mut Walker, frames: usize) {
    w.press(KeyCode::KeyW);
    w.run(frames);
    w.release(KeyCode::KeyW);
}

/// Two yards out from the Goldshire inn's north wall, where it runs flat.
const INN_WALL_START: [f32; 3] = {
    let ([nx, ny], c) = INN_WALL;
    let y = 25.5;
    [(c - ny * y) / nx + 2.0 * nx, y + 2.0 * ny, 56.572]
};

const SCENARIOS: [Scenario; 7] = [
    Scenario {
        name: "the abbey stairs",
        feet: [-8908.6, -190.5, 82.5],
        heading_deg: 270.0,
        walk: |w| {
            walk_path(w, &ABBEY_STAIRS, HZ, 8.0);
        },
    },
    Scenario {
        name: "off the abbey's gallery",
        feet: [-8906.0, -189.0, 89.17],
        heading_deg: 0.0,
        walk: |w| hold_w(w, 150),
    },
    Scenario {
        name: "a hundred and fifty yards down to the meadow",
        feet: [MEADOW[0], MEADOW[1], 59.86 + 150.0],
        heading_deg: 0.0,
        walk: |w| {
            w.run(330);
        },
    },
    Scenario {
        name: "the canal swim",
        feet: [CANAL_RAMP[0], CANAL_RAMP[1], 95.38],
        heading_deg: DOWN_THE_RAMP,
        walk: |w| {
            w.press(KeyCode::KeyW);
            w.run(120);
            for _ in 0..420 {
                let [x, y, _] = w.wow();
                w.aim((CANAL_RAMP[1] - y).atan2(CANAL_RAMP[0] - x).to_degrees());
                w.run(1);
            }
        },
    },
    Scenario {
        name: "Crystal Lake",
        feet: [SHORE[0], SHORE[1], 59.87],
        heading_deg: 180.0,
        walk: |w| {
            w.press(KeyCode::KeyW);
            w.run(300);
            w.aim(0.0);
            w.run(420);
        },
    },
    Scenario {
        name: "along the inn's wall",
        feet: INN_WALL_START,
        heading_deg: 218.0,
        walk: |w| hold_w(w, 72),
    },
    Scenario {
        name: "up a steep bank",
        feet: [HILLSIDE[0], HILLSIDE[1], 98.26],
        heading_deg: 311.9,
        walk: |w| hold_w(w, 300),
    },
];

#[test]
fn an_honest_client_walking_the_scenarios_is_never_put_back() {
    let mut refused = Vec::new();
    for walk in &SCENARIOS {
        let server = serve(&[(walk.feet, walk.heading_deg)]);
        let look = CharacterLook::naked(1, 0);
        let Some(mut w) = Walker::joined(server.addr(), "Walker", look, HZ) else {
            return;
        };
        (walk.walk)(&mut w);
        w.run(30);
        let net = w.net().expect("still joined");
        let (claims, corrections) = (net.claims_sent(), net.corrections());
        drop(w);
        let summary = server.stop().expect("the server stops");
        let why: Vec<String> = Why::ALL
            .iter()
            .zip(summary.refused)
            .filter(|&(_, n)| n > 0)
            .map(|(why, n)| format!("{n} {why:?}"))
            .collect();
        eprintln!(
            "{}: {claims} claims, {corrections} corrections, refused: [{}]",
            walk.name,
            why.join(", ")
        );
        if corrections > 0 || !why.is_empty() {
            refused.push(walk.name);
        }
    }
    assert!(refused.is_empty(), "put back on {refused:?}");
}
