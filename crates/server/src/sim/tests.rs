use protocol::{Appearance, Claim, Hello, Movement, Record, ServerMessage, flags};
use tokio::sync::mpsc::UnboundedReceiver;

use super::*;
use crate::net::Outbox;
use crate::world::Input;

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a pool")
}

fn join(conn: u32) -> Stamped {
    Stamped {
        conn,
        seq: 0,
        received_ms: 0,
        input: Input::Join(Hello {
            version: VERSION,
            name: format!("p{conn}"),
            appearance: Appearance::default(),
        }),
    }
}

fn claim(conn: u32, seq: u32, received_ms: u32, ack: u32, movement: Movement) -> Stamped {
    Stamped {
        conn,
        seq,
        received_ms,
        input: Input::Claim(Claim { ack, movement }),
    }
}

fn running(time: u32, pos: [f32; 3]) -> Movement {
    Movement {
        time,
        flags: flags::FORWARD,
        pos,
        ..Movement::default()
    }
}

fn spawn(x: f32, y: f32) -> Spawn {
    Spawn {
        pos: [x, y, 0.0],
        facing: 0.0,
    }
}

/// Each player walks straight out from its spawn, some claiming several times a tick, and every
/// eleventh teleports once and then takes its correction.
fn crowd_inputs(players: u32, ticks: u32) -> Vec<Vec<Stamped>> {
    let mut all = vec![(0..players).map(join).collect::<Vec<_>>()];
    for t in 1..ticks {
        let mut inputs = Vec::new();
        for p in 0..players {
            let per_tick = 1 + p % 3;
            let liar = p % 11 == 0;
            let ack = u32::from(liar && t > 40);
            for k in 0..per_tick {
                let time = t * 50 + k * 15;
                let d = time as f32 / 1000.0 * 6.5;
                let angle = p as f32 * 0.3;
                let mut pos = [p as f32 * 3.0 + d * angle.cos(), d * angle.sin(), 0.0];
                if liar && t == 40 && k == 0 {
                    pos[1] += 80.0;
                }
                inputs.push(claim(p, t * 4 + k, time, ack, running(time, pos)));
            }
        }
        all.push(inputs);
    }
    all
}

fn hashes(threads: usize, order: InputOrder, inputs: &[Vec<Stamped>]) -> (Vec<u64>, u32) {
    let spawns = (0..60).map(|p| spawn(p as f32 * 3.0, 0.0)).collect();
    let mut sim = Sim::new(spawns, Rules::default(), View::default(), 0, 50);
    let pool = pool(threads);
    let mut refused = 0;
    let hashes = inputs
        .iter()
        .map(|tick| {
            let st = sim.tick(&pool, tick, order, None, true);
            refused += st.refused.iter().sum::<u32>();
            st.hash
        })
        .collect();
    (hashes, refused)
}

#[test]
fn the_same_inputs_give_the_same_world_on_one_thread_and_on_four() {
    let inputs = crowd_inputs(60, 120);
    let (one, refused) = hashes(1, InputOrder::Canonical, &inputs);
    let (four, _) = hashes(4, InputOrder::Canonical, &inputs);
    assert_eq!(one, four);
    assert_eq!(refused, 6, "each liar is refused once, then acknowledges");
}

#[test]
fn inputs_applied_out_of_order_change_the_world() {
    let inputs = crowd_inputs(60, 120);
    let (canonical, _) = hashes(1, InputOrder::Canonical, &inputs);
    let (racy, _) = hashes(4, InputOrder::Racy, &inputs);
    assert_ne!(canonical.last(), racy.last());
}

#[derive(Debug, PartialEq)]
enum Got {
    Appear(u32),
    Vanish(u32),
    Move(u32),
    Correct(u32),
}

/// The records of tick `tick`'s batch, the next message unadmitted on `rx`.
fn next_batch(rx: &mut UnboundedReceiver<Vec<u8>>, tick: u32) -> Vec<Got> {
    let bytes = rx.try_recv().expect("a batch every tick");
    let Ok(ServerMessage::Batch(b)) = ServerMessage::read(&bytes[protocol::LEN_BYTES..]) else {
        panic!("not a batch");
    };
    assert_eq!(b.tick, tick);
    b.map(|r| match r.expect("a valid record") {
        Record::Appear { id, .. } => Got::Appear(id),
        Record::Vanish { id } => Got::Vanish(id),
        Record::Move { id, .. } => Got::Move(id),
        Record::Correct { seq, .. } => Got::Correct(seq),
    })
    .collect()
}

fn moves_of(records: &[Got], of: u32) -> usize {
    records.iter().filter(|r| **r == Got::Move(of)).count()
}

#[test]
fn an_observer_sees_the_near_at_once_the_far_slowly_and_never_a_refused_claim() {
    let spawns = vec![
        spawn(0.0, 0.0),
        spawn(10.0, 0.0),
        spawn(150.0, 0.0),
        spawn(60.0, 0.0),
    ];
    let mut sim = Sim::new(spawns, Rules::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let mut rx = Vec::new();
    for conn in 0..4 {
        let (outbox, r) = Outbox::channel();
        shared.hold_outbox(conn, outbox);
        rx.push(r);
    }
    let pool = pool(2);
    let mut run =
        |inputs: Vec<Stamped>| sim.tick(&pool, &inputs, InputOrder::Canonical, Some(&shared), true);
    run((0..4).map(join).collect());
    let first: Vec<Vec<Got>> = rx
        .iter_mut()
        .map(|r| {
            assert!(r.try_recv().is_ok(), "a welcome");
            next_batch(r, 0)
        })
        .collect();
    assert_eq!(first[0], [Got::Appear(1), Got::Appear(3)]);
    let mut far_moves = 0;
    for t in 1..=21 {
        let mut inputs = vec![claim(
            3,
            t,
            t * 50,
            0,
            running(t * 50, [60.0, t as f32 * 0.3, 0.0]),
        )];
        if t == 1 {
            inputs.push(claim(1, 1, 50, 0, running(50, [10.0, 0.0, 0.0])));
        }
        if t == 3 {
            inputs.push(claim(2, 1, 150, 0, running(150, [1.0, 0.0, 0.0])));
        }
        run(inputs);
        let records = next_batch(&mut rx[0], t);
        assert_eq!(moves_of(&records, 1), usize::from(t == 1), "tick {t}");
        assert!(!records.contains(&Got::Appear(2)));
        far_moves += moves_of(&records, 3);
        let own = next_batch(&mut rx[2], t);
        assert_eq!(own.contains(&Got::Correct(1)), t == 3, "tick {t}");
    }
    assert_eq!(
        far_moves, 3,
        "the flag change, then one refresh per ten ticks"
    );
    run(vec![Stamped {
        conn: 1,
        seq: 2,
        received_ms: 1100,
        input: Input::Leave,
    }]);
    assert!(next_batch(&mut rx[0], 22).contains(&Got::Vanish(1)));
}

#[test]
fn a_client_that_falls_behind_gets_flag_changes_but_no_refreshes_until_it_catches_up() {
    let spawns = vec![spawn(0.0, 0.0), spawn(10.0, 0.0)];
    let mut sim = Sim::new(spawns, Rules::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let (slow, mut rx) = Outbox::channel();
    let saw = slow.seen_by();
    shared.hold_outbox(0, slow);
    shared.hold_outbox(1, Outbox::channel().0);
    let pool = pool(1);
    let mut run =
        |inputs: Vec<Stamped>| sim.tick(&pool, &inputs, InputOrder::Canonical, Some(&shared), true);
    run((0..2).map(join).collect());
    assert!(rx.try_recv().is_ok(), "a welcome");
    assert_eq!(next_batch(&mut rx, 0), [Got::Appear(1)]);
    saw(0);
    let mut moved = Vec::new();
    for t in 1..=30 {
        if t == 26 {
            saw(25);
        }
        let flags = if (20..25).contains(&t) {
            0
        } else {
            flags::FORWARD
        };
        let x = 10.0 + t.min(20) as f32 * 0.3;
        let movement = Movement {
            time: t * 50,
            flags,
            pos: [x, 0.0, 0.0],
            ..Movement::default()
        };
        run(vec![claim(1, t, t * 50, 0, movement)]);
        if moves_of(&next_batch(&mut rx, t), 1) > 0 {
            moved.push(t);
        }
    }
    let caught_up: Vec<u32> = (1..=10).chain([20, 25]).chain(26..=30).collect();
    assert_eq!(
        moved, caught_up,
        "ten ticks behind it gets only the stop and the start"
    );
}
