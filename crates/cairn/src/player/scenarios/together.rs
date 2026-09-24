use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use protocol::flags;
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
const STATE_TIMEOUT: Duration = Duration::from_secs(10);

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
        super::walker::ready(&mut [&mut w]);
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

/// The movement flags of the other player's copy.
fn the_others_flags(p: &mut Painter) -> Option<u32> {
    let world = p.app.world_mut();
    world
        .query_filtered::<&RemoteMotion, With<OtherPlayer>>()
        .iter(world)
        .next()
        .map(|m| m.flags)
}

/// Runs the painter until its copy of the other player moves as `now` says.
fn wait_until_the_other(p: &mut Painter, what: &str, now: impl Fn(u32) -> bool) {
    let deadline = Instant::now() + STATE_TIMEOUT;
    while !the_others_flags(p).is_some_and(&now) {
        assert!(
            Instant::now() < deadline,
            "the other player was never seen {what}"
        );
        wait(p, 0.0);
    }
}

fn shoot_the_other(p: &mut Painter, name: &str) {
    let across = yards_to_the_other(p);
    assert!(
        across.is_some_and(|yd| yd < SUBJECT_WITHIN_YD),
        "{name}: the other player is {across:?} yd away"
    );
    p.shoot(name);
}

struct Scene {
    name: &'static str,
    painter: (Stand, CharacterLook),
    runner: (Stand, CharacterLook),
    night: bool,
}

/// Goldshire's crossroads: the runner starts ahead of the painter and to its right, and runs
/// across its view.
fn in_goldshire(name: &'static str, painter: CharacterLook, runner: CharacterLook) -> Scene {
    Scene {
        name,
        painter: (
            Stand {
                feet: [GOLDSHIRE[0], GOLDSHIRE[1], 57.0],
                heading_deg: EAST,
            },
            painter,
        ),
        runner: (
            Stand {
                feet: [GOLDSHIRE[0] - 5.0, GOLDSHIRE[1] - 6.0, 57.0],
                heading_deg: 0.0,
            },
            runner,
        ),
        night: false,
    }
}

/// The snow outside Kharanos, where a runner's feet print and a dwarf's breath shows.
fn on_the_snow(painter: CharacterLook, runner: CharacterLook) -> Scene {
    Scene {
        name: "together-snow",
        painter: (
            Stand {
                feet: [-5650.0, -450.0, 393.0],
                heading_deg: 0.0,
            },
            painter,
        ),
        runner: (
            Stand {
                feet: [-5644.0, -455.0, 395.1],
                heading_deg: 90.0,
            },
            runner,
        ),
        night: false,
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn two_players_see_each_other_run_and_jump_in_goldshire_by_day_and_at_night() {
    let human = CharacterLook::naked(1, 0);
    let orc = CharacterLook::naked(2, 1);
    let dwarf = CharacterLook::naked(3, 0);
    let scenes = [
        in_goldshire("together-day", human.clone(), orc.clone()),
        Scene {
            night: true,
            ..in_goldshire("together-night", orc, human.clone())
        },
        on_the_snow(human, dwarf),
    ];
    for scene in scenes {
        let name = scene.name;
        let (at, painter) = scene.painter;
        let (start, running) = scene.runner;
        let server = serve(&[at, start]);
        let Some(mut p) = Painter::joined(server.addr(), at.feet, at.heading_deg, painter) else {
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
        if scene.night {
            p.set_time(0, 30);
        }
        p.orbit(0.0, 6.0);
        wait(&mut p, 2.5);
        shoot_the_other(&mut p, &format!("{name}-1-standing"));
        let _ = r.cue.send(());
        let running = |f: u32| f & flags::FORWARD != 0 && f & flags::FALLING == 0;
        wait_until_the_other(&mut p, "running", running);
        wait(&mut p, 0.3);
        shoot_the_other(&mut p, &format!("{name}-2-running"));
        wait_until_the_other(&mut p, "in the air", |f| f & flags::FALLING != 0);
        wait(&mut p, 0.2);
        shoot_the_other(&mut p, &format!("{name}-3-jumping"));
        wait_until_the_other(&mut p, "landed", |f| f & flags::FALLING == 0);
        wait(&mut p, 0.8);
        shoot_the_other(&mut p, &format!("{name}-4-landed"));
        p.tilt_up(-0.6);
        wait(&mut p, 0.3);
        shoot_the_other(&mut p, &format!("{name}-5-the-ground-it-ran-over"));
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
