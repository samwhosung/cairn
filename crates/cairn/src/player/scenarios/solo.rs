use bevy::input::keyboard::KeyCode;
use protocol::Why;
use server::{InputOrder, Replay, Replicate, Summary};
use world::coords::wow_to_bevy;
use world::unit::CharacterLook;

use super::honest::{Stand, serve};
use super::pair::{self, MEADOW_WALK, Script};
use super::walker::Walker;
use super::{MEADOW, horizontal};
use crate::player::Mode;
use crate::player::state::Player;

const HZ: f32 = 60.0;
const FAR_HILLSIDE: [f32; 2] = [MEADOW[0] + 500.0, MEADOW[1]];
const ABOVE: f32 = 10.0;

fn put_back(w: &mut Walker) -> [f32; 3] {
    for _ in 0..(5.0 * HZ) as usize {
        if w.net().is_some_and(|n| n.corrections() > 0) {
            w.settle();
            return w.wow();
        }
        w.run(1);
    }
    panic!("the body was never put back");
}

fn run_on(w: &mut Walker) {
    w.run(120);
    w.press(KeyCode::KeyW);
    w.run(90);
    w.release(KeyCode::KeyW);
    w.run(30);
}

#[test]
fn alone_a_landing_five_hundred_yards_off_is_granted_and_walked_on_from() {
    let Some(mut w) = Walker::on_ground(MEADOW, 0.0, HZ) else {
        return;
    };
    let stood = w.wow();
    let ground = w.fly_over(FAR_HILLSIDE);
    w.land([FAR_HILLSIDE[0], FAR_HILLSIDE[1], ground + ABOVE]);
    w.settle();
    run_on(&mut w);
    let end = w.wow();
    let judged = w.stop_and_judge().expect("its own server");
    eprintln!(
        "landed {:.1} yd from where it stood, ran on to {end:?}",
        horizontal(stood, end)
    );
    assert!(judged.honest(), "{judged:?}");
    assert_eq!(judged.teleports, 2, "stood on the meadow, then landed");
    assert!(judged.claims > 4, "{judged:?}");
    assert!(
        horizontal(end, [FAR_HILLSIDE[0], FAR_HILLSIDE[1], 0.0]) < 15.0,
        "{end:?}"
    );
}

#[test]
fn a_landing_on_a_server_that_does_not_grant_it_is_put_back_and_told_why() {
    let stand = Stand {
        feet: [MEADOW[0], MEADOW[1], 59.86],
        heading_deg: 0.0,
    };
    let server = serve(&[stand]);
    let Some(mut w) = Walker::joined(server.addr(), "Guest", CharacterLook::naked(1, 0), HZ) else {
        return;
    };
    let stood = w.wow();
    let ground = w.fly_over(FAR_HILLSIDE);
    w.land([FAR_HILLSIDE[0], FAR_HILLSIDE[1], ground + ABOVE]);
    let back = put_back(&mut w);
    run_on(&mut w);
    let net = w.net().expect("still joined");
    let (teleports, corrections, told) =
        (net.teleports_sent(), net.corrections(), net.why_put_back());
    drop(w);
    let summary = server.stop().expect("the server stops");
    eprintln!(
        "put back {:.4} yd from where it stood, told {told:?}; refused {:?}",
        horizontal(stood, back),
        summary.refused
    );
    assert_eq!((teleports, corrections, told), (1, 1, Some(Why::Teleport)));
    assert!(horizontal(stood, back) < 0.01, "{back:?} against {stood:?}");
    let mut refused = [0; Why::ALL.len()];
    refused[Why::Teleport as usize] = 1;
    assert_eq!(
        summary.refused, refused,
        "only the landing, and nothing after it"
    );
}

#[test]
fn a_body_moved_without_asking_is_put_back() {
    let Some(mut w) = Walker::on_ground(MEADOW, 0.0, HZ) else {
        return;
    };
    let stood = w.wow();
    let ground = w.fly_over(FAR_HILLSIDE);
    let world = w.app.world_mut();
    *world.resource_mut::<Mode>() = Mode::Walk;
    let mut player = world.resource_mut::<Player>();
    player.pos = wow_to_bevy([FAR_HILLSIDE[0], FAR_HILLSIDE[1], ground + ABOVE]);
    player.settling = true;
    let back = put_back(&mut w);
    run_on(&mut w);
    let judged = w.stop_and_judge().expect("its own server");
    eprintln!(
        "moved without asking: {judged:?}, put back {:.4} yd from where it stood",
        horizontal(stood, back)
    );
    assert_eq!(judged.refused, [(Why::Speed, 1)]);
    assert_eq!(judged.corrections, 1);
    assert!(horizontal(stood, back) < 0.01, "{back:?} against {stood:?}");
}

#[test]
fn a_solo_walk_with_a_landing_replays_to_the_same_world_at_every_tick() {
    let dir = std::env::temp_dir().join(format!("cairn-solo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a temp dir");
    let log = dir.join("inputs.log");
    let Some(w) = Walker::recorded([MEADOW[0], MEADOW[1], 500.0], 0.0, HZ, log.clone()) else {
        return;
    };
    let mut w = w.grounded(MEADOW);
    let ground = w.fly_over(FAR_HILLSIDE);
    w.land([FAR_HILLSIDE[0], FAR_HILLSIDE[1], ground + ABOVE]);
    w.settle();
    run_on(&mut w);
    let judged = w.stop_and_judge().expect("its own server");
    assert!(judged.honest() && judged.teleports == 2, "{judged:?}");
    let how = Replay {
        threads: 1,
        order: InputOrder::Canonical,
        keep_refusals: true,
        replicate: Replicate::No,
        actions: true,
        keeping_at: None,
    };
    let replayed = server::replay(&log, &how).expect("a replay");
    eprintln!(
        "replayed {} ticks to {:016x}: first mismatch {:?}",
        replayed.ticks, replayed.hash, replayed.first_mismatch
    );
    assert_eq!(replayed.first_mismatch, None);
    assert!(replayed.ticks > 100 && replayed.refusals.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "a measurement, for a release build; set WOW_DATA"]
fn the_server_cost_of_one_walk_alone_and_of_the_same_over_loopback() {
    let place = &MEADOW_WALK;
    let Some(mut w) = Walker::new("Azeroth", place.a.feet, place.a.heading_deg, HZ) else {
        return;
    };
    let mut script = Script::default();
    for frame in 0..place.frames {
        script.frame(&mut w, place.acts, frame);
        w.run(1);
    }
    let alone = w.stop_and_judge().expect("its own server");
    let over_loopback = pair::alone(place).expect("the install");
    eprintln!("{}", Summary::header());
    eprintln!("{}", alone.summary.row("one player alone, in-process"));
    eprintln!("{}", over_loopback.row("one player over loopback"));
}
