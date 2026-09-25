//! Landing from flight, which the server is asked for.

use bevy::input::keyboard::KeyCode;
use protocol::Why;
use world::unit::CharacterLook;

use super::honest::{Stand, serve};
use super::walker::Walker;
use super::{MEADOW, horizontal};

const HZ: f32 = 60.0;
/// Five hundred yards north of the meadow, over open ground.
const FAR: [f32; 2] = [MEADOW[0] + 500.0, MEADOW[1]];
const ABOVE: f32 = 10.0;

/// Runs until the server's correction has come and the body settled where it put it; where
/// that is.
fn put_back(w: &mut Walker) -> [f32; 3] {
    for _ in 0..(5.0 * HZ) as usize {
        if w.net().is_some_and(|n| n.corrections() > 0) {
            break;
        }
        w.run(1);
    }
    w.settle();
    w.wow()
}

fn run_on(w: &mut Walker) {
    w.run(120);
    w.press(KeyCode::KeyW);
    w.run(90);
    w.release(KeyCode::KeyW);
    w.run(30);
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
    let ground = w.fly_over(FAR);
    w.land([FAR[0], FAR[1], ground + ABOVE]);
    let back = put_back(&mut w);
    run_on(&mut w);
    let net = w.net().expect("still joined");
    let (teleports, corrections, told) = (net.teleports_sent(), net.corrections(), net.told());
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
