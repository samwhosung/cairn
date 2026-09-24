//! Two players drawn from each other's windows on the GPU: one runs and jumps in Goldshire while
//! the other's follow camera looks on, by day and at night, each a different race.

use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use world::unit::{BodyDressed, CharacterLook, UnitBody};

use super::honest::serve;
use super::pictures::{EAST, GOLDSHIRE, Painter};
use super::walker::Walker;
use crate::net::Remote;

const HZ: f32 = 60.0;
const STEP: Duration = Duration::from_nanos(16_666_667);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);

/// When the runner presses and lets go of what, from its cue.
const RUN_AND_JUMP: [(f32, KeyCode, bool); 4] = [
    (0.0, KeyCode::KeyW, true),
    (0.85, KeyCode::Space, true),
    (0.9, KeyCode::Space, false),
    (1.3, KeyCode::KeyW, false),
];

/// The other player's client, on a thread of its own so that a shot never stalls it: it settles,
/// says so, and on its cue runs and jumps on the wall clock until the painter hangs up.
fn runner(server: SocketAddr, look: CharacterLook) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
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
        for (at, key, down) in RUN_AND_JUMP {
            while go.elapsed().as_secs_f32() < at {
                w.run(1);
            }
            if down {
                w.press(key);
            } else {
                w.release(key);
            }
        }
        while matches!(cued.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            w.run(1);
        }
    });
    (is_ready, cue)
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

/// Every other player the painter sees is dressed, and its skins have loaded.
fn others_dressed(p: &mut Painter) -> bool {
    let world = p.app.world_mut();
    let bodies: Vec<UnitBody> = world
        .query_filtered::<&UnitBody, (With<Remote>, With<BodyDressed>)>()
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

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn two_players_see_each_other_run_and_jump_in_goldshire_by_day_and_at_night() {
    let human = CharacterLook::naked(1, 0);
    let orc = CharacterLook::naked(2, 1);
    for (name, painter, running, night) in [
        ("together-day", human.clone(), orc.clone(), false),
        ("together-night", orc, human, true),
    ] {
        let at = [GOLDSHIRE[0], GOLDSHIRE[1], 57.0];
        let ahead_and_right = [GOLDSHIRE[0] - 5.0, GOLDSHIRE[1] - 6.0, 57.0];
        let server = serve(&[(at, EAST), (ahead_and_right, 0.0)]);
        let Some(mut p) = Painter::joined(server.addr(), at, EAST, painter) else {
            return;
        };
        let (ready, cue) = runner(server.addr(), running);
        let deadline = Instant::now() + LOAD_TIMEOUT;
        let mut settled = false;
        while !(settled && p.arrived() && others_dressed(&mut p)) {
            assert!(Instant::now() < deadline, "the pair never arrived");
            settled |= ready.try_recv().is_ok();
            wait(&mut p, 0.0);
        }
        if night {
            p.set_time(0, 30);
        }
        p.orbit(0.0, 6.0);
        wait(&mut p, 2.5);
        p.shoot(&format!("{name}-1-standing"));
        let _ = cue.send(());
        let go = Instant::now();
        for (at, shot) in [(0.7, "2-running"), (1.15, "3-jumping"), (2.6, "4-landed")] {
            wait(&mut p, at - go.elapsed().as_secs_f32());
            p.shoot(&format!("{name}-{shot}"));
        }
        let _ = cue.send(());
    }
}
