//! The window hosts melee and fights another player until it dies and rises, each shot checking
//! what it shows; without the show, every check fails.

use std::f32::consts::TAU;
use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bevy::animation::transition::AnimationTransitions;
use bevy::input::ButtonState;
use bevy::input::keyboard::KeyCode;
use bevy::prelude::*;
use protocol::flags;
use server::Spawn;
use world::rig::ModelAnimations;
use world::unit::{CharacterLook, UnitShow};

use super::pictures::{EAST, GOLDSHIRE, Painter};
use super::together::{others_dressed_and_skinned, wait};
use super::walker::{Walker, ready};
use crate::net::{self, Net, OtherPlayer, RemoteMotion};
use crate::player::PlayerBody;

const HZ: f32 = 60.0;
const LOAD_TIMEOUT: Duration = Duration::from_secs(300);
const RISE_TIMEOUT: Duration = Duration::from_secs(90);
/// Three blows of 40 kill, and the dead lie long enough for a slow frame to catch them.
const KNOBS: &str = "damage_min = 40\ndamage_max = 40\nrespawn_s = 30\n";
const STAND: u16 = 0;
const DEATH: u16 = 1;
const DEAD: u16 = 6;
const COMBAT_WOUND: u16 = 9;
const ATTACK_UNARMED: u16 = 16;
const NORTH: f32 = 0.0;
const WEST: f32 = 90.0;

enum Cue {
    Swing,
    Done,
}

struct Fighter {
    ready: mpsc::Receiver<()>,
    cue: mpsc::Sender<Cue>,
}

/// The other player's client, on a thread of its own: it faces west and swings when cued.
fn fighter(server: SocketAddr, look: CharacterLook) -> Fighter {
    let (ready_tx, ready_rx) = mpsc::channel();
    let (cue, cued) = mpsc::channel();
    thread::spawn(move || {
        let Some(mut w) = Walker::welcomed(server, "Fighter", look, HZ) else {
            return;
        };
        w.aim(WEST);
        ready(&mut [&mut w]);
        let _ = ready_tx.send(());
        loop {
            match cued.try_recv() {
                Ok(Cue::Swing) => {
                    w.tap(KeyCode::Digit1);
                }
                Ok(Cue::Done) | Err(mpsc::TryRecvError::Disconnected) => return,
                Err(mpsc::TryRecvError::Empty) => {
                    w.run(1);
                }
            }
        }
    });
    Fighter {
        ready: ready_rx,
        cue,
    }
}

/// What a body plays: its game's show, the clip its driver runs, and whether that has played out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Plays {
    show: UnitShow,
    clip: Option<u16>,
    played_out: bool,
}

fn plays<F: bevy::ecs::query::QueryFilter>(p: &mut Painter) -> Option<Plays> {
    let world = p.app.world_mut();
    let mut q = world.query_filtered::<(
        &UnitShow,
        &AnimationTransitions,
        &ModelAnimations,
        &AnimationPlayer,
    ), F>();
    let (show, tr, anims, player) = q.iter(world).next()?;
    let node = tr.get_main_animation();
    let clip = node.and_then(|n| anims.clips.iter().find(|c| c.node == n));
    Some(Plays {
        show: *show,
        clip: clip.map(|c| c.anim_id),
        played_out: node
            .and_then(|n| player.animation(n))
            .is_some_and(bevy::animation::ActiveAnimation::is_finished),
    })
}

fn the_other(p: &mut Painter) -> Option<Plays> {
    plays::<With<OtherPlayer>>(p)
}

fn its_own(p: &mut Painter) -> Option<Plays> {
    plays::<With<PlayerBody>>(p)
}

fn where_the_other_stands(p: &mut Painter) -> Option<([f32; 3], f32, u32)> {
    let world = p.app.world_mut();
    let mut q = world.query_filtered::<&RemoteMotion, With<OtherPlayer>>();
    q.iter(world)
        .next()
        .map(|m| (m.wow_pos, m.orientation, m.flags))
}

struct Checked {
    shot: String,
    held: bool,
    saw: String,
}

/// The shots whose check did not come out `held`, with what each saw.
fn unlike(checked: &[Checked], held: bool) -> Vec<String> {
    checked
        .iter()
        .filter(|c| c.held != held)
        .map(|c| format!("{}: {}", c.shot, c.saw))
        .collect()
}

struct Fight {
    p: Painter,
    name: &'static str,
    checked: Vec<Checked>,
}

impl Fight {
    fn shoot(&mut self, shot: &str, check: impl FnOnce(&mut Painter) -> (bool, String)) -> bool {
        let shot = format!("{}-{shot}", self.name);
        self.p.shoot(&shot);
        let (held, saw) = check(&mut self.p);
        eprintln!("{shot}: {} ({saw})", if held { "holds" } else { "fails" });
        self.checked.push(Checked { shot, held, saw });
        held
    }

    fn swing(&mut self) {
        self.p.key(KeyCode::Digit1, ButtonState::Pressed);
        self.p.run(1);
        self.p.key(KeyCode::Digit1, ButtonState::Released);
    }
}

fn plays_clip(who: Option<Plays>, clip: u16) -> (bool, String) {
    (
        who.is_some_and(|w| w.clip == Some(clip)),
        format!("{who:?}"),
    )
}

/// The window, a human, stands in Goldshire facing east; the other, an orc, three yards east of
/// it, spawned facing north and turned to face it.
fn fight(name: &'static str, show: bool) -> Option<Vec<Checked>> {
    let feet = [GOLDSHIRE[0], GOLDSHIRE[1], 57.0];
    let across = [GOLDSHIRE[0], GOLDSHIRE[1] - 3.0, 57.0];
    let over = game::KnobsFile::parse(KNOBS, "the fight").expect("knobs");
    let cfg = server::Config {
        game: Some(catalog::load("melee", None, &over.lines, 0).expect("melee")),
        spawns: vec![
            Spawn {
                pos: feet,
                facing: EAST.to_radians(),
            },
            Spawn {
                pos: across,
                facing: NORTH.to_radians(),
            },
        ],
        ..net::own_server(Some(0), 0, feet, EAST.to_radians())
    };
    let mut p = Painter::hosting(cfg, feet, EAST, CharacterLook::naked(1, 0))?;
    let net = p.app.world_mut().resource_mut::<Net>();
    let addr = net.hosted_addr().expect("the window hosts");
    net.into_inner().faults().no_show = !show;
    let other = fighter(addr, CharacterLook::naked(2, 0));
    let deadline = Instant::now() + LOAD_TIMEOUT;
    let mut settled = false;
    p.clock().pause();
    while !(settled && p.arrived() && others_dressed_and_skinned(&mut p)) {
        assert!(Instant::now() < deadline, "the fighters never arrived");
        settled |= other.ready.try_recv().is_ok();
        wait(&mut p, 0.0);
    }
    p.clock().unpause();
    p.keep_time_by_its_frames();
    p.level_with_the_wall();
    p.orbit(0.0, 6.0);
    p.wait(2.0);
    let mut f = Fight {
        p,
        name,
        checked: Vec::new(),
    };

    // Until the dead shot every wait is a count of frames, so the world has aged alike in every
    // run when it is taken.
    let _ = other.cue.send(Cue::Swing);
    f.p.wait(0.4);
    f.shoot("1-the-other-swings", |p| {
        plays_clip(the_other(p), ATTACK_UNARMED)
    });
    f.p.wait(1.6);
    f.swing();
    f.p.wait(0.4);
    f.shoot("2-the-window-swings", |p| {
        plays_clip(its_own(p), ATTACK_UNARMED)
    });
    f.p.wait(2.1);
    f.swing();
    f.p.wait(0.3);
    f.shoot("3-the-other-is-hit", |p| {
        plays_clip(the_other(p), COMBAT_WOUND)
    });
    f.p.wait(2.2);
    f.swing();
    f.p.wait(4.0);
    // Up close the window's own body fades out: its stance after its swings hangs on when the
    // server's word of them came, and the dead pose does not.
    f.p.orbit(0.0, 0.0);
    f.p.wait(1.0);
    let lay_dead = f.shoot("4-the-other-lies-dead", |p| {
        let other = the_other(p);
        let held = other
            .is_some_and(|o| o.show.pose == Some(DEAD) && o.clip == Some(DEATH) && o.played_out);
        (held, format!("{other:?}"))
    });

    f.p.orbit(0.0, 6.0);
    f.p.level_with_the_wall();
    let placed = |stands: Option<([f32; 3], f32, u32)>| {
        stands.is_some_and(|(pos, facing, fl)| {
            let turned = (facing - NORTH.to_radians()).rem_euclid(TAU);
            let at = (pos[0] - across[0]).hypot(pos[1] - across[1]);
            fl & flags::ROOT == 0 && at < 0.05 && turned.min(TAU - turned) < 0.05
        })
    };
    let risen_by = Instant::now() + RISE_TIMEOUT;
    while !placed(where_the_other_stands(&mut f.p)) {
        assert!(Instant::now() < risen_by, "the other never rose");
        f.p.run(1);
    }
    f.p.wait(1.5);
    f.shoot("5-the-other-stands-at-its-spawn", |p| {
        let (other, stands) = (the_other(p), where_the_other_stands(p));
        let at_spawn = placed(stands);
        let stands_again = other.is_some_and(|o| o.show.pose.is_none() && o.clip == Some(STAND));
        let held = lay_dead && stands_again && at_spawn;
        (
            held,
            format!("{other:?} at {stands:?}, lay dead {lay_dead}"),
        )
    });
    let _ = other.cue.send(Cue::Done);
    Some(f.checked)
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn the_window_hosts_melee_and_fights_another_until_it_dies_and_rises() {
    let Some(checked) = fight("fight", true) else {
        return;
    };
    let failed = unlike(&checked, true);
    assert!(failed.is_empty(), "{failed:#?}");
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn without_the_show_every_shot_of_the_fight_fails_its_check() {
    let Some(checked) = fight("fight-unshown", false) else {
        return;
    };
    let held = unlike(&checked, false);
    assert!(held.is_empty(), "{held:#?}");
}
