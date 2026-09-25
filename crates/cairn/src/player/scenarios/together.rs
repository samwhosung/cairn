use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::animation::transition::AnimationTransitions;
use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use bevy::time::Real;
use protocol::flags;
use world::coords::bevy_to_wow;
use world::rig::ModelAnimations;
use world::unit::{BodyDressed, CharacterLook, UnitBody, UnitMotion};

use super::clock::SharedClock;
use super::honest::{Stand, serve};
use super::painter::{Painter, frame_costs, rig_census};
use super::pair::Act;
use super::pictures::{EAST, GOLDSHIRE, ON_THE_SNOW_OUTSIDE_KHARANOS};
use super::walker::{Walker, ready};
use crate::net::{OtherPlayer, RemoteMotion};
use crate::player::state::Player;

const HZ: f32 = 60.0;
const STEP: Duration = Duration::from_nanos(16_666_667);
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
const SUBJECT_WITHIN_YD: f32 = 12.0;
const STATE_TIMEOUT: Duration = Duration::from_secs(10);
const DRESSED_STEADY_FOR: Duration = Duration::from_secs(10);
const CROWD_WATCHED: Duration = Duration::from_secs(60);
const CROWD_FRAMES: usize = 900;
const MOVING_OVER_YD_PER_S: f32 = 1.0;
const STAND_ANIM: u16 = 0;

const RUN_AND_JUMP: [(f32, Act); 4] = [
    (0.0, Act::Press(KeyCode::KeyW)),
    (0.85, Act::Press(KeyCode::Space)),
    (0.9, Act::Release(KeyCode::Space)),
    (1.3, Act::Release(KeyCode::KeyW)),
];

struct Runner {
    w: Walker,
    go: Option<Duration>,
    next: usize,
}

impl Runner {
    fn joined(clock: &SharedClock, look: CharacterLook) -> Self {
        let mut w = Walker::welcomed(clock, "Runner", look).expect("the install");
        ready(&mut [&mut w]);
        Self {
            w,
            go: None,
            next: 0,
        }
    }

    fn go(&mut self) {
        self.go = Some(self.game_time());
    }

    fn game_time(&self) -> Duration {
        self.w.app.world().resource::<Time<Virtual>>().elapsed()
    }

    fn frame(&mut self) {
        if let Some(go) = self.go {
            while let Some(&(at, act)) = RUN_AND_JUMP.get(self.next)
                && self.game_time().saturating_sub(go).as_secs_f32() >= at
            {
                match act {
                    Act::Press(key) => self.w.press(key),
                    Act::Release(key) => self.w.release(key),
                    _ => {}
                }
                self.next += 1;
            }
        }
        self.w.run(1);
    }
}

pub(super) fn wait(p: &mut Painter, secs: f32) {
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

/// Frames until the other player is in view, then frames that hold the clock until the painter's
/// world has arrived and the other is dressed, with the painter's game clock held throughout.
pub(super) fn arrive(p: &mut Painter) {
    let deadline = Instant::now() + LOAD_TIMEOUT;
    p.clock().pause();
    while yards_to_the_other(p).is_none() {
        assert!(
            Instant::now() < deadline,
            "the other player never came into view"
        );
        p.run(1);
    }
    while !(p.arrived() && others_dressed_and_skinned(p)) {
        assert!(Instant::now() < deadline, "the pair never arrived");
        p.hold();
    }
    p.clock().unpause();
}

pub(super) fn others_dressed_and_skinned(p: &mut Painter) -> bool {
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

fn yards_to_the_other(p: &mut Painter) -> Option<f32> {
    let world = p.app.world_mut();
    let me = bevy_to_wow(world.resource::<Player>().pos);
    world
        .query_filtered::<&RemoteMotion, With<OtherPlayer>>()
        .iter(world)
        .map(|m| (m.wow_pos[0] - me[0]).hypot(m.wow_pos[1] - me[1]))
        .reduce(f32::min)
}

fn the_others_flags(p: &mut Painter) -> Option<u32> {
    let world = p.app.world_mut();
    world
        .query_filtered::<&RemoteMotion, With<OtherPlayer>>()
        .iter(world)
        .next()
        .map(|m| m.flags)
}

fn wait_until_the_other(p: &mut Painter, what: &str, flags_say: impl Fn(u32) -> bool) {
    let deadline = p.clock().elapsed() + STATE_TIMEOUT;
    while !the_others_flags(p).is_some_and(&flags_say) {
        assert!(
            p.clock().elapsed() < deadline,
            "the other player was never seen {what}"
        );
        p.run(1);
    }
}

fn shoot_the_other(p: &mut Painter, name: &str, flags_say: impl Fn(u32) -> bool) {
    let across = yards_to_the_other(p);
    assert!(
        across.is_some_and(|yd| yd < SUBJECT_WITHIN_YD),
        "{name}: the other player is {across:?} yd away"
    );
    p.shoot(name);
    assert!(
        the_others_flags(p).is_some_and(flags_say),
        "{name}: the other player had moved on before the shot was taken"
    );
}

struct Scene {
    name: &'static str,
    painter: (Stand, CharacterLook),
    runner: (Stand, CharacterLook),
    night: bool,
}

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

fn on_the_snow(painter: CharacterLook, runner: CharacterLook) -> Scene {
    let snow = ON_THE_SNOW_OUTSIDE_KHARANOS;
    Scene {
        name: "together-snow",
        painter: (
            Stand {
                feet: [snow.xy[0], snow.xy[1], 393.0],
                heading_deg: snow.heading,
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
        let clock = serve(&[at, start], HZ);
        let Some(mut p) = Painter::on_clock(&clock, at.feet, at.heading_deg, painter) else {
            return;
        };
        let runner = Rc::new(RefCell::new(Runner::joined(&clock, running)));
        let beside = runner.clone();
        p.beside(move || beside.borrow_mut().frame());
        arrive(&mut p);
        if scene.night {
            p.set_time(0, 30);
        }
        p.orbit(0.0, 6.0);
        p.wait(2.5);
        let standing = |f: u32| f == 0;
        let running = |f: u32| f & flags::FORWARD != 0 && f & flags::FALLING == 0;
        let in_the_air = |f: u32| f & flags::FALLING != 0;
        let landed = |f: u32| f & flags::FALLING == 0;
        shoot_the_other(&mut p, &format!("{name}-1-standing"), standing);
        runner.borrow_mut().go();
        wait_until_the_other(&mut p, "running", running);
        p.wait(0.3);
        shoot_the_other(&mut p, &format!("{name}-2-running"), running);
        wait_until_the_other(&mut p, "in the air", in_the_air);
        p.wait(0.5);
        shoot_the_other(&mut p, &format!("{name}-3-jumping"), in_the_air);
        wait_until_the_other(&mut p, "landed", landed);
        p.wait(0.8);
        shoot_the_other(&mut p, &format!("{name}-4-landed"), landed);
        p.tilt_up(-0.6);
        p.wait(0.3);
        shoot_the_other(&mut p, &format!("{name}-5-the-ground-it-ran-over"), landed);
        drop(p);
        drop(runner);
        clock.borrow_mut().stop();
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

fn joined_to_the_crowd() -> Option<Painter> {
    let Some(server) = std::env::var("CAIRN_SERVER")
        .ok()
        .and_then(|a| a.parse().ok())
    else {
        eprintln!("skipped: set CAIRN_SERVER");
        return None;
    };
    let at = [GOLDSHIRE[0], GOLDSHIRE[1], 57.0];
    Painter::joined(server, at, EAST, CharacterLook::naked(1, 0))
}

#[test]
#[ignore = "a measurement, for a release build on a GPU; set WOW_DATA, CAIRN_PICTURES, \
            CAIRN_SERVER to a server a crowd walks and CAIRN_CROWD to how many walk it"]
fn the_frame_cost_standing_beside_a_crowd() {
    let Some(mut p) = joined_to_the_crowd() else {
        return;
    };
    let walking: usize = std::env::var("CAIRN_CROWD")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(50);
    let deadline = Instant::now() + LOAD_TIMEOUT;
    while !(p.arrived() && crowd(&mut p).dressed >= walking) {
        assert!(Instant::now() < deadline, "the crowd never arrived");
        wait(&mut p, 0.0);
    }
    wait(&mut p, 5.0);
    let Crowd { seen, dressed } = crowd(&mut p);
    eprintln!("measuring {CROWD_FRAMES} frames");
    let standing = frame_costs(&mut p, CROWD_FRAMES);
    eprintln!("measured");
    let rigs = rig_census(&mut p);
    eprintln!("beside a crowd of {seen} ({dressed} dressed): standing {standing} with {rigs}");
}

#[derive(Default)]
struct Drawn {
    frames: u32,
    moving: f64,
    playing_stand: f64,
    at_speed_zero: f64,
    unskinned: f64,
    unskinned_units: HashSet<Entity>,
}

fn look_at_the_crowd(p: &mut Painter, last: &mut HashMap<Entity, Vec2>, d: &mut Drawn) {
    let world = p.app.world_mut();
    let dt = world.resource::<Time<Real>>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    d.frames += 1;
    let mut q = world.query_filtered::<(
        Entity,
        &Transform,
        &UnitMotion,
        Option<&BodyDressed>,
        Option<&AnimationTransitions>,
        Option<&ModelAnimations>,
    ), With<OtherPlayer>>();
    for (e, t, motion, dressed, tr, anims) in q.iter(world) {
        let at = Vec2::new(t.translation.x, t.translation.z);
        let Some(was) = last.insert(e, at) else {
            continue;
        };
        if at.distance(was) / dt <= MOVING_OVER_YD_PER_S {
            continue;
        }
        let dt = f64::from(dt);
        d.moving += dt;
        let clip = tr
            .and_then(AnimationTransitions::get_main_animation)
            .and_then(|node| anims?.clips.iter().find(|c| c.node == node))
            .map(|c| c.anim_id);
        if clip == Some(STAND_ANIM) {
            d.playing_stand += dt;
        }
        if motion.speed == 0.0 {
            d.at_speed_zero += dt;
        }
        if dressed.is_some_and(|b| b.slot == 0) {
            d.unskinned += dt;
            d.unskinned_units.insert(e);
        }
    }
}

#[test]
#[ignore = "a measurement, for a release build on a GPU; set WOW_DATA, CAIRN_PICTURES and \
            CAIRN_SERVER to a server a crowd walks"]
fn how_a_walking_crowd_is_drawn() {
    let Some(mut p) = joined_to_the_crowd() else {
        return;
    };
    let deadline = Instant::now() + LOAD_TIMEOUT;
    let (mut most, mut since) = (0, Instant::now());
    while !(p.arrived() && most > 0 && since.elapsed() > DRESSED_STEADY_FOR) {
        assert!(Instant::now() < deadline, "the crowd never arrived");
        wait(&mut p, 0.0);
        let dressed = crowd(&mut p).dressed;
        if dressed > most {
            (most, since) = (dressed, Instant::now());
        }
    }
    let Crowd { seen, dressed } = crowd(&mut p);
    let (mut last, mut d) = (HashMap::new(), Drawn::default());
    let until = Instant::now() + CROWD_WATCHED;
    while Instant::now() < until {
        let next = Instant::now() + STEP;
        p.app.update();
        look_at_the_crowd(&mut p, &mut last, &mut d);
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let pct = |x: f64| 100.0 * x / d.moving.max(f64::MIN_POSITIVE);
    eprintln!(
        "a crowd of {seen} ({dressed} dressed) watched {} s over {} frames: drawn moving {:.1} \
         unit-s, playing Stand {:.1}%, at speed 0 {:.1}%, unskinned {:.1}% by {} units",
        CROWD_WATCHED.as_secs(),
        d.frames,
        d.moving,
        pct(d.playing_stand),
        pct(d.at_speed_zero),
        pct(d.unskinned),
        d.unskinned_units.len(),
    );
}
