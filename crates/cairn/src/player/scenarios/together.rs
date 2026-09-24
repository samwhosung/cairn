use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use world::coords::bevy_to_wow;
use world::unit::{BodyDressed, CharacterLook, UnitBody};

use super::honest::{Stand, serve};
use super::pair::Act;
use super::pictures::{EAST, GOLDSHIRE, Painter, frame_costs};
use super::walker::Walker;
use crate::net::{OtherPlayer, RemoteMotion};
use crate::player::state::Player;

const HZ: f32 = 60.0;
const STEP: Duration = Duration::from_nanos(16_666_667);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// The runner starts 8 yd from the painter and runs past it.
const SUBJECT_WITHIN_YD: f32 = 12.0;

const RUN_AND_JUMP: [(f32, Act); 4] = [
    (0.0, Act::Press(KeyCode::KeyW)),
    (0.85, Act::Press(KeyCode::Space)),
    (0.9, Act::Release(KeyCode::Space)),
    (1.3, Act::Release(KeyCode::KeyW)),
];

struct Runner {
    ready: mpsc::Receiver<()>,
    cue: mpsc::Sender<()>,
}

/// The other player's client, on a thread of its own so that a shot never stalls it.
fn runner(server: SocketAddr, look: CharacterLook) -> Runner {
    let (ready, is_ready) = mpsc::channel();
    let (cue, cued) = mpsc::channel::<()>();
    thread::spawn(move || {
        let Some(mut w) = Walker::welcomed(server, "Runner", look, HZ) else {
            return;
        };
        w.settle();
        w.pace();
        let _ = ready.send(());
        while cued.try_recv().is_err() {
            w.run(1);
        }
        let go = Instant::now();
        for (at, act) in RUN_AND_JUMP {
            while go.elapsed().as_secs_f32() < at {
                w.run(1);
            }
            match act {
                Act::Press(key) => w.press(key),
                Act::Release(key) => w.release(key),
                _ => {}
            }
        }
        while matches!(cued.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            w.run(1);
        }
    });
    Runner {
        ready: is_ready,
        cue,
    }
}

fn wait(p: &mut Painter, secs: f32) {
    let until = Instant::now() + Duration::from_secs_f32(secs.max(0.0));
    loop {
        let next = Instant::now() + STEP;
        p.app.update();
        thread::sleep(next.saturating_duration_since(Instant::now()));
        if Instant::now() >= until {
            return;
        }
    }
}

fn others_dressed_and_skinned(p: &mut Painter) -> bool {
    let world = p.app.world_mut();
    let bodies: Vec<UnitBody> = world
        .query_filtered::<&UnitBody, (With<OtherPlayer>, With<BodyDressed>)>()
        .iter(world)
        .cloned()
        .collect();
    let images = world.resource::<Assets<Image>>();
    !bodies.is_empty()
        && bodies.iter().flat_map(|b| &b.character).all(|c| {
            [&c.body, &c.hair, &c.skin_extra]
                .into_iter()
                .flatten()
                .all(|h| images.contains(h))
        })
}

/// How far across from the painter's body the other player's copy stands.
fn yards_to_the_other(p: &mut Painter) -> Option<f32> {
    let world = p.app.world_mut();
    let me = bevy_to_wow(world.resource::<Player>().pos);
    world
        .query_filtered::<&RemoteMotion, With<OtherPlayer>>()
        .iter(world)
        .map(|m| (m.wow_pos[0] - me[0]).hypot(m.wow_pos[1] - me[1]))
        .reduce(f32::min)
}

fn shoot_the_other(p: &mut Painter, name: &str) {
    let across = yards_to_the_other(p);
    assert!(
        across.is_some_and(|yd| yd < SUBJECT_WITHIN_YD),
        "{name}: the other player is {across:?} yd away"
    );
    p.shoot(name);
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn two_players_see_each_other_run_and_jump_in_goldshire_by_day_and_at_night() {
    let human = CharacterLook::naked(1, 0);
    let orc = CharacterLook::naked(2, 1);
    for (name, painter, running, night) in [
        ("together-day", human.clone(), orc.clone(), false),
        ("together-night", orc, human, true),
    ] {
        let at = Stand {
            feet: [GOLDSHIRE[0], GOLDSHIRE[1], 57.0],
            heading_deg: EAST,
        };
        let ahead_and_right = Stand {
            feet: [GOLDSHIRE[0] - 5.0, GOLDSHIRE[1] - 6.0, 57.0],
            heading_deg: 0.0,
        };
        let server = serve(&[at, ahead_and_right]);
        let Some(mut p) = Painter::joined(server.addr(), at.feet, EAST, painter) else {
            return;
        };
        let r = runner(server.addr(), running);
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let mut settled = false;
        while !(settled && p.arrived() && others_dressed_and_skinned(&mut p)) {
            assert!(Instant::now() < deadline, "the pair never arrived");
            settled |= r.ready.try_recv().is_ok();
            wait(&mut p, 0.0);
        }
        if night {
            p.set_time(0, 30);
        }
        p.orbit(0.0, 6.0);
        wait(&mut p, 2.5);
        shoot_the_other(&mut p, &format!("{name}-1-standing"));
        let _ = r.cue.send(());
        let go = Instant::now();
        for (at, shot) in [(0.7, "2-running"), (1.15, "3-jumping"), (2.6, "4-landed")] {
            wait(&mut p, at - go.elapsed().as_secs_f32());
            shoot_the_other(&mut p, &format!("{name}-{shot}"));
        }
        let _ = r.cue.send(());
        drop(p);
        server.stop().expect("the server stops");
    }
}

struct Crowd {
    seen: usize,
    dressed: usize,
}

fn crowd(p: &mut Painter) -> Crowd {
    let world = p.app.world_mut();
    let seen = world
        .query_filtered::<(), With<OtherPlayer>>()
        .iter(world)
        .count();
    let dressed = world
        .query_filtered::<(), (With<OtherPlayer>, With<BodyDressed>)>()
        .iter(world)
        .count();
    Crowd { seen, dressed }
}

#[test]
#[ignore = "a measurement, for a release build on a GPU; set WOW_DATA, CAIRN_PICTURES and \
            CAIRN_SERVER to a server a crowd walks"]
fn the_frame_cost_beside_a_crowd() {
    let Some(server) = std::env::var("CAIRN_SERVER")
        .ok()
        .and_then(|a| a.parse().ok())
    else {
        eprintln!("skipped: set CAIRN_SERVER");
        return;
    };
    let at = [GOLDSHIRE[0], GOLDSHIRE[1], 57.0];
    let Some(mut p) = Painter::joined(server, at, EAST, CharacterLook::naked(1, 0)) else {
        return;
    };
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while !(p.arrived() && crowd(&mut p).dressed >= 50) {
        assert!(Instant::now() < deadline, "the crowd never arrived");
        wait(&mut p, 0.0);
    }
    wait(&mut p, 5.0);
    let Crowd { seen, dressed } = crowd(&mut p);
    let standing = frame_costs(&mut p, 600);
    p.key(KeyCode::KeyW, bevy::input::ButtonState::Pressed);
    let running = frame_costs(&mut p, 1200);
    eprintln!(
        "beside a crowd of {seen} ({dressed} dressed): standing {standing}; running {running}"
    );
}
