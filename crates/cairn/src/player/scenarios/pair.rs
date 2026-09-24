//! Two clients and a server in one process, each judging its copy of the other every frame.

use std::f32::consts::{PI, TAU};

use bevy::input::keyboard::KeyCode;
use world::unit::CharacterLook;

use super::MEADOW;
use super::honest::{Stand, serve};
use super::walker::{Walker, ready};
use crate::net::{Faults, OtherPlayer, RemoteMotion};
use crate::player::state::{GRAVITY, RUN_SPEED};

pub const HZ: f32 = 60.0;
/// The wait for a tick, the batch's building and delivery, the watcher's frame and its replay
/// buffer.
const CLAIM_TO_VIEW_SECS: f32 = 0.15;
const NEAR_TIER_SECS: f32 = 0.05;
const HEARTBEAT_SECS: f32 = protocol::HEARTBEAT_MS as f32 / 1000.0;
const HEARD_LATE_SECS: f32 = CLAIM_TO_VIEW_SECS + NEAR_TIER_SECS;
const REVERSAL: f32 = 2.0 * RUN_SPEED;
pub const BOUND_ACROSS: f32 = REVERSAL * HEARD_LATE_SECS + STEP_ACROSS;
/// Up, a fall without a jump is heard only at the next heartbeat.
pub const BOUND_UP: f32 =
    0.5 * GRAVITY * (HEARTBEAT_SECS + HEARD_LATE_SECS) * (HEARTBEAT_SECS + HEARD_LATE_SECS)
        + STEP_UP;
const STEP_ACROSS: f32 = 1.0 / 128.0;
const STEP_UP: f32 = 1.0 / 32.0;
const STEP_FACING: f32 = TAU / 256.0;
const REST_FRAMES: u32 = 30;

#[derive(Clone, Copy, Debug)]
pub enum Act {
    Press(KeyCode),
    Release(KeyCode),
    Aim(f32),
    Turn { deg_per_frame: f32 },
    Pitch(f32),
}

#[derive(Default)]
pub struct Script {
    turn: f32,
}

impl Script {
    pub fn frame(&mut self, w: &mut Walker, acts: &[(u32, Act)], frame: u32) {
        for &(_, act) in acts.iter().filter(|(at, _)| *at == frame) {
            match act {
                Act::Press(k) => w.press(k),
                Act::Release(k) => w.release(k),
                Act::Aim(deg) => w.aim(deg),
                Act::Turn { deg_per_frame } => self.turn = deg_per_frame,
                Act::Pitch(deg) => w.pitch(deg),
            }
        }
        if self.turn != 0.0 {
            let heading = w.player().face_yaw.to_degrees();
            w.aim(heading + self.turn);
        }
    }
}

#[derive(Debug, Default)]
pub struct Watch {
    pub frames: u32,
    pub worst_across: f32,
    pub worst_up: f32,
    pub over: u32,
    pub missing: u32,
    pub rest_frames: u32,
    pub rest_across: f32,
    pub rest_up: f32,
    pub rest_facing: f32,
    still: u32,
    last: Option<([f32; 3], f32)>,
    worst_at: Option<(u32, u32)>,
}

pub fn copy_of(observer: &mut Walker, id: u32) -> Option<([f32; 3], f32)> {
    let world = observer.app.world_mut();
    world
        .query::<(&OtherPlayer, &RemoteMotion)>()
        .iter(world)
        .find(|(r, _)| r.id == id)
        .map(|(_, m)| (m.wow_pos, m.orientation))
}

fn wrap(angle: f32) -> f32 {
    PI - (PI - angle).rem_euclid(TAU)
}

impl Watch {
    pub fn judge(&mut self, observer: &mut Walker, truth: &Walker, id: u32, frame: u32) {
        let (pos, facing) = (truth.wow(), truth.player().face_yaw);
        let moving =
            protocol::flags::ANY_MOVE | protocol::flags::TURNING | protocol::flags::FALLING;
        let resting = truth.player().move_flags & moving == 0;
        let same = self.last == Some((pos, facing));
        self.still = if resting && same { self.still + 1 } else { 0 };
        self.last = Some((pos, facing));
        let Some((copy, orientation)) = copy_of(observer, id) else {
            self.missing += 1;
            return;
        };
        self.frames += 1;
        let across = (copy[0] - pos[0]).hypot(copy[1] - pos[1]);
        let up = (copy[2] - pos[2]).abs();
        if across > self.worst_across {
            self.worst_at = Some((frame, truth.player().move_flags));
        }
        self.worst_across = self.worst_across.max(across);
        self.worst_up = self.worst_up.max(up);
        if across > BOUND_ACROSS || up > BOUND_UP {
            self.over += 1;
        }
        if self.still >= REST_FRAMES {
            self.rest_frames += 1;
            self.rest_across = self.rest_across.max(across);
            self.rest_up = self.rest_up.max(up);
            self.rest_facing = self.rest_facing.max(wrap(orientation - facing).abs());
        }
    }

    pub fn within_bounds(&self) -> bool {
        self.over == 0 && self.frames > 0
    }

    pub fn at_rest_to_the_step(&self) -> bool {
        self.rest_frames > 0
            && self.rest_across <= STEP_ACROSS * 2f32.sqrt()
            && self.rest_up <= STEP_UP
            && self.rest_facing <= STEP_FACING
    }

    pub fn line(&self, who: &str) -> String {
        format!(
            "{who}: {} frames, worst {:.3} yd across (bound {BOUND_ACROSS:.2}) and {:.3} up \
             (bound {BOUND_UP:.2}) at {:?}, {} over, {} missing; at rest {} frames, worst \
             {:.4} across, {:.4} up, {:.2}° facing",
            self.frames,
            self.worst_across,
            self.worst_up,
            self.worst_at,
            self.over,
            self.missing,
            self.rest_frames,
            self.rest_across,
            self.rest_up,
            self.rest_facing.to_degrees()
        )
    }
}

pub struct Place {
    pub name: &'static str,
    pub a: Stand,
    pub b: Stand,
    pub acts: &'static [(u32, Act)],
    pub delay_b: u32,
    pub frames: u32,
}

pub const MEADOW_WALK: Place = Place {
    name: "runs, turns by mouse and by key, strafes, backs up, jumps and walks on open meadow",
    a: Stand {
        feet: [MEADOW[0], MEADOW[1], 59.86],
        heading_deg: 0.0,
    },
    b: Stand {
        feet: [MEADOW[0], MEADOW[1] - 2.5, 59.86],
        heading_deg: 0.0,
    },
    acts: &[
        (0, Act::Press(KeyCode::KeyW)),
        (
            90,
            Act::Turn {
                deg_per_frame: -1.0,
            },
        ),
        (180, Act::Turn { deg_per_frame: 0.0 }),
        (240, Act::Release(KeyCode::KeyW)),
        (270, Act::Turn { deg_per_frame: 1.5 }),
        (330, Act::Turn { deg_per_frame: 0.0 }),
        (360, Act::Press(KeyCode::KeyA)),
        (420, Act::Release(KeyCode::KeyA)),
        (450, Act::Press(KeyCode::KeyQ)),
        (510, Act::Release(KeyCode::KeyQ)),
        (510, Act::Press(KeyCode::KeyE)),
        (570, Act::Release(KeyCode::KeyE)),
        (600, Act::Press(KeyCode::KeyS)),
        (660, Act::Release(KeyCode::KeyS)),
        (690, Act::Press(KeyCode::KeyW)),
        (690, Act::Press(KeyCode::KeyD)),
        (750, Act::Release(KeyCode::KeyD)),
        (780, Act::Press(KeyCode::Space)),
        (781, Act::Release(KeyCode::Space)),
        (790, Act::Release(KeyCode::KeyW)),
        (900, Act::Press(KeyCode::Space)),
        (901, Act::Release(KeyCode::Space)),
        (960, Act::Press(KeyCode::NumpadDivide)),
        (961, Act::Release(KeyCode::NumpadDivide)),
        (961, Act::Press(KeyCode::KeyW)),
        (1020, Act::Release(KeyCode::KeyW)),
        (1020, Act::Press(KeyCode::NumpadDivide)),
        (1021, Act::Release(KeyCode::NumpadDivide)),
    ],
    delay_b: 60,
    frames: 1140,
};

pub const CANAL: Place = Place {
    name: "off a Stormwind canal's west quay into the water, and swims, turns, dives, rises, \
           strafes and hops",
    a: Stand {
        feet: [-8778.0, 515.4, 97.8],
        heading_deg: 0.0,
    },
    b: Stand {
        feet: [-8778.0, 518.0, 97.8],
        heading_deg: 0.0,
    },
    acts: &[
        (0, Act::Press(KeyCode::KeyW)),
        (
            180,
            Act::Turn {
                deg_per_frame: -1.0,
            },
        ),
        (270, Act::Turn { deg_per_frame: 0.0 }),
        (270, Act::Pitch(-20.0)),
        (360, Act::Pitch(20.0)),
        (450, Act::Pitch(0.0)),
        (450, Act::Press(KeyCode::KeyQ)),
        (510, Act::Release(KeyCode::KeyQ)),
        (540, Act::Release(KeyCode::KeyW)),
        (570, Act::Press(KeyCode::Space)),
        (571, Act::Release(KeyCode::Space)),
        (630, Act::Aim(180.0)),
        (630, Act::Press(KeyCode::KeyW)),
        (780, Act::Release(KeyCode::KeyW)),
    ],
    delay_b: 60,
    frames: 900,
};

pub struct Walked {
    pub a_seen_by_b: Watch,
    pub b_seen_by_a: Watch,
    pub frames_until_unlisted: Option<u32>,
    pub frames_until_faded: Option<u32>,
    pub corrections_a: u32,
    pub corrections_b: u32,
    pub server: server::Summary,
}

pub fn walk(place: &Place, looks: [CharacterLook; 2], b_faults: Faults) -> Option<Walked> {
    let server = serve(&[place.a, place.b]);
    let [look_a, look_b] = looks;
    let mut a = Walker::welcomed(server.addr(), "A", look_a, HZ)?;
    let mut b = Walker::welcomed(server.addr(), "B", look_b, HZ)?;
    let id = |w: &Walker| w.net().and_then(|n| n.welcome()).map(|w| w.id);
    let (id_a, id_b) = (id(&a).expect("A joined"), id(&b).expect("B joined"));
    *b.net_mut().expect("B joined").faults() = b_faults;
    ready(&mut [&mut a, &mut b]);
    let (mut script_a, mut script_b) = (Script::default(), Script::default());
    let mut walked = Walked {
        a_seen_by_b: Watch::default(),
        b_seen_by_a: Watch::default(),
        frames_until_unlisted: None,
        frames_until_faded: None,
        corrections_a: 0,
        corrections_b: 0,
        server: server::Summary::default(),
    };
    for frame in 0..place.frames {
        script_a.frame(&mut a, place.acts, frame);
        if let Some(f) = frame.checked_sub(place.delay_b) {
            script_b.frame(&mut b, place.acts, f);
        }
        a.run(1);
        b.run(1);
        walked.a_seen_by_b.judge(&mut b, &a, id_a, frame);
        walked.b_seen_by_a.judge(&mut a, &b, id_b, frame);
    }
    let corrected = |w: &Walker| w.net().map_or(0, crate::net::Net::corrections);
    (walked.corrections_a, walked.corrections_b) = (corrected(&a), corrected(&b));
    drop(a);
    for frame in 0..(4.0 * HZ) as u32 {
        b.run(1);
        if walked.frames_until_unlisted.is_none() && copy_of(&mut b, id_a).is_none() {
            walked.frames_until_unlisted = Some(frame);
        }
        let world = b.app.world_mut();
        let bodies = world.query::<&RemoteMotion>().iter(world).count();
        if walked.frames_until_faded.is_none() && bodies == 0 {
            walked.frames_until_faded = Some(frame);
        }
    }
    drop(b);
    walked.server = server.stop().expect("the server stops");
    Some(walked)
}

pub fn alone(place: &Place) -> Option<server::Summary> {
    let server = serve(&[place.a]);
    let mut a = Walker::joined(server.addr(), "A", CharacterLook::naked(1, 0), HZ)?;
    let mut script = Script::default();
    for frame in 0..place.frames {
        script.frame(&mut a, place.acts, frame);
        a.run(1);
    }
    drop(a);
    Some(server.stop().expect("the server stops"))
}

fn human() -> CharacterLook {
    CharacterLook::naked(1, 0)
}

/// Runs the test `name` of this module again in a process of its own, where its clients share
/// the engine's task pools with no other test's, and fails if it fails there. `true` in that
/// process.
fn alone_in_a_process(name: &str) -> bool {
    const ALONE: &str = "CAIRN_TEST_ALONE";
    let module = module_path!().split_once("::").map_or("", |(_, m)| m);
    let test = format!("{module}::{name}");
    if std::env::var(ALONE).is_ok_and(|t| t == test) {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args([test.as_str(), "--exact", "--nocapture"])
        .env(ALONE, &test)
        .output()
        .expect("the test runs in a process of its own");
    print!("{}", String::from_utf8_lossy(&out.stdout));
    eprint!("{}", String::from_utf8_lossy(&out.stderr));
    assert!(
        out.status.success(),
        "{test} failed in a process of its own"
    );
    false
}

#[test]
fn two_clients_see_each_other_walk_run_turn_jump_fall_and_swim() {
    if !alone_in_a_process("two_clients_see_each_other_walk_run_turn_jump_fall_and_swim") {
        return;
    }
    for place in [&MEADOW_WALK, &CANAL] {
        let Some(w) = walk(place, [human(), human()], Faults::default()) else {
            return;
        };
        eprintln!("{}:", place.name);
        eprintln!("  {}", w.a_seen_by_b.line("B's copy of A"));
        eprintln!("  {}", w.b_seen_by_a.line("A's copy of B"));
        eprintln!(
            "  A left: no longer a player after {:?} frames, its body gone after {:?}; \
             corrections to A {} and B {}",
            w.frames_until_unlisted, w.frames_until_faded, w.corrections_a, w.corrections_b
        );
        for watch in [&w.a_seen_by_b, &w.b_seen_by_a] {
            assert!(watch.within_bounds(), "{}", place.name);
            assert!(watch.at_rest_to_the_step(), "{}", place.name);
        }
        assert_eq!((w.corrections_a, w.corrections_b), (0, 0), "{}", place.name);
        assert!(
            w.frames_until_unlisted.is_some_and(|f| f < 30),
            "{}",
            place.name
        );
        assert!(
            w.frames_until_faded.is_some_and(|f| f < 180),
            "{}",
            place.name
        );
    }
}

#[test]
fn a_view_that_drops_every_other_move_or_never_dead_reckons_breaks_the_bound() {
    let controls = [
        Faults {
            drop_every_other: true,
            ..Faults::default()
        },
        Faults {
            no_dead_reckoning: true,
            ..Faults::default()
        },
    ];
    for faults in controls {
        let Some(w) = walk(&MEADOW_WALK, [human(), human()], faults) else {
            return;
        };
        eprintln!("{faults:?}: {}", w.a_seen_by_b.line("B's copy of A"));
        assert!(w.a_seen_by_b.over > 0, "{faults:?} passed the check");
    }
}

#[test]
#[ignore = "a measurement, for a release build; set WOW_DATA"]
fn the_server_cost_of_one_player_and_of_two() {
    let Some(one) = alone(&MEADOW_WALK) else {
        return;
    };
    let two = walk(&MEADOW_WALK, [human(), human()], Faults::default()).expect("the install");
    eprintln!("{}", server::Summary::header());
    eprintln!("{}", one.row("one player in-process"));
    eprintln!("{}", two.server.row("two players in-process"));
}
