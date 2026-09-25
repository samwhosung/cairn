use std::io;
use std::net::SocketAddr;

use bevy::input::keyboard::KeyCode;
use server::{Config, Running, Spawn, Summary, Why};
use world::unit::CharacterLook;

use super::clock::{self, Served};
use super::walker::Walker;
use super::{
    ABBEY_STAIRS, CANAL_RAMP, DOWN_THE_RAMP, HILLSIDE, INN_WALL_START, MEADOW, SHORE, walk_path,
};

const HZ: f32 = 60.0;

#[derive(Clone, Copy, Debug)]
pub struct Stand {
    pub feet: [f32; 3],
    pub heading_deg: f32,
}

pub struct LoopbackServer(Running);

impl LoopbackServer {
    pub fn addr(&self) -> SocketAddr {
        self.0.addr().expect("the server listens")
    }

    pub fn stop(self) -> io::Result<Summary> {
        self.0.stop()
    }
}

pub fn config(stands: &[Stand]) -> Config {
    Config {
        tick_threads: 1,
        io_threads: 1,
        spawns: stands
            .iter()
            .map(|s| Spawn {
                pos: s.feet,
                facing: s.heading_deg.to_radians(),
            })
            .collect(),
        ..Config::default()
    }
}

pub fn serve_over_loopback(stands: &[Stand]) -> LoopbackServer {
    LoopbackServer(server::start(config(stands)).expect("a server"))
}

/// A server whose players join beside `stands` in turn, on a clock their windows step `hz` times
/// a second.
pub fn serve(stands: &[Stand], hz: f32) -> Served {
    clock::serve(&config(stands), clock::step_at(hz))
}

struct Scenario {
    name: &'static str,
    at: Stand,
    walk: fn(&mut Walker),
}

fn hold_w(w: &mut Walker, frames: usize) {
    w.press(KeyCode::KeyW);
    w.run(frames);
    w.release(KeyCode::KeyW);
}

const SCENARIOS: [Scenario; 7] = [
    Scenario {
        name: "the abbey stairs",
        at: Stand {
            feet: [-8908.6, -190.5, 82.5],
            heading_deg: 270.0,
        },
        walk: |w| {
            walk_path(w, &ABBEY_STAIRS, HZ, 8.0);
        },
    },
    Scenario {
        name: "off a Stormwind canal's west quay into the water",
        at: Stand {
            feet: [-8778.0, 515.4, 97.8],
            heading_deg: 0.0,
        },
        walk: |w| hold_w(w, 150),
    },
    Scenario {
        name: "a hundred and fifty yards down to the meadow",
        at: Stand {
            feet: [MEADOW[0], MEADOW[1], 59.86 + 150.0],
            heading_deg: 0.0,
        },
        walk: |w| {
            w.run(330);
        },
    },
    Scenario {
        name: "the canal swim",
        at: Stand {
            feet: [CANAL_RAMP[0], CANAL_RAMP[1], 95.38],
            heading_deg: DOWN_THE_RAMP,
        },
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
        at: Stand {
            feet: [SHORE[0], SHORE[1], 59.87],
            heading_deg: 180.0,
        },
        walk: |w| {
            w.press(KeyCode::KeyW);
            w.run(300);
            w.aim(0.0);
            w.run(420);
        },
    },
    Scenario {
        name: "along the inn's wall",
        at: Stand {
            feet: [INN_WALL_START[0], INN_WALL_START[1], 56.572],
            heading_deg: 218.0,
        },
        walk: |w| hold_w(w, 72),
    },
    Scenario {
        name: "up a steep bank",
        at: Stand {
            feet: [HILLSIDE[0], HILLSIDE[1], 98.26],
            heading_deg: 311.9,
        },
        walk: |w| hold_w(w, 300),
    },
];

#[test]
fn an_honest_client_walking_the_scenarios_is_never_put_back() {
    let mut refused = Vec::new();
    for walk in &SCENARIOS {
        let clock = serve(&[walk.at], HZ);
        let Some(mut w) = Walker::joined(&clock, "Walker", CharacterLook::naked(1, 0)) else {
            return;
        };
        (walk.walk)(&mut w);
        w.run(30);
        let net = w.net().expect("still joined");
        let (claims, corrections) = (net.claims_sent(), net.corrections());
        drop(w);
        let summary = clock.borrow_mut().stop();
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
