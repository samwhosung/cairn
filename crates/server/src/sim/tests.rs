use std::collections::HashMap;

use libm::{cosf, sinf};
use protocol::{Appearance, Claim, Hello, Movement, Record, ServerMessage, Why, flags};
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
        nth: 0,
        received_ms: 0,
        input: Input::Join(Hello {
            version: VERSION,
            name: format!("p{conn}"),
            appearance: Appearance::default(),
        }),
    }
}

fn host_join(conn: u32) -> Stamped {
    Stamped {
        input: Input::HostJoin(Hello {
            version: VERSION,
            name: "Host".into(),
            appearance: Appearance::default(),
        }),
        ..join(conn)
    }
}

fn claim(conn: u32, nth: u32, received_ms: u32, ack: u32, movement: Movement) -> Stamped {
    Stamped {
        conn,
        nth,
        received_ms,
        input: Input::Claim(Claim { ack, movement }),
    }
}

fn teleport(conn: u32, nth: u32, received_ms: u32, movement: Movement) -> Stamped {
    Stamped {
        input: Input::Teleport(Claim { ack: 0, movement }),
        ..claim(conn, nth, received_ms, 0, movement)
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
                let mut pos = [p as f32 * 3.0 + d * cosf(angle), d * sinf(angle), 0.0];
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
    let mut sim = Sim::new(spawns, Limits::default(), View::default(), 0, 50);
    let pool = pool(threads);
    let nobody = Shared::new();
    let mut refused = 0;
    let hashes = inputs
        .iter()
        .map(|tick| {
            let st = sim.tick(&pool, tick, order, Batches::Send(&nobody));
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
    Correct(u32, Why),
    Granted([f32; 3]),
}

struct Client {
    rx: UnboundedReceiver<Vec<u8>>,
    slots: HashMap<u16, u32>,
}

impl Client {
    fn new(rx: UnboundedReceiver<Vec<u8>>) -> Self {
        Self {
            rx,
            slots: HashMap::new(),
        }
    }

    fn welcome(&mut self) {
        assert!(self.rx.try_recv().is_ok(), "a welcome");
    }

    fn next_batch(&mut self, tick: u32) -> Vec<Got> {
        let bytes = self.rx.try_recv().expect("a batch every tick");
        let Ok(ServerMessage::Batch(b)) = ServerMessage::read(&bytes[protocol::LEN_BYTES..]) else {
            panic!("not a batch");
        };
        assert_eq!(b.tick, tick);
        let mut got = Vec::new();
        for r in b {
            got.push(match r.expect("a valid record") {
                Record::Appear { slot, id, .. } => {
                    assert!(self.slots.insert(slot, id).is_none(), "slot {slot} taken");
                    Got::Appear(id)
                }
                Record::Vanish { slot } => Got::Vanish(self.slots.remove(&slot).expect("held")),
                Record::Move { slot, .. }
                | Record::Turn { slot, .. }
                | Record::State { slot, .. } => Got::Move(self.slots[&slot]),
                Record::Correct { seq, why, .. } => Got::Correct(seq, why),
                Record::Granted { movement } => Got::Granted(movement.pos),
                Record::Place { .. } | Record::Game { .. } | Record::Show { .. } => {
                    unreachable!("no game runs here")
                }
            });
        }
        got
    }
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
    let mut sim = Sim::new(spawns, Limits::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let mut rx = Vec::new();
    for conn in 0..4 {
        let (outbox, r) = Outbox::channel();
        shared.hold_outbox(conn, outbox);
        rx.push(Client::new(r));
    }
    let pool = pool(2);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run((0..4).map(join).collect());
    let first: Vec<Vec<Got>> = rx
        .iter_mut()
        .map(|r| {
            r.welcome();
            r.next_batch(0)
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
        let records = rx[0].next_batch(t);
        assert_eq!(moves_of(&records, 1), usize::from(t == 1), "tick {t}");
        assert!(!records.contains(&Got::Appear(2)));
        far_moves += moves_of(&records, 3);
        let own = rx[2].next_batch(t);
        assert_eq!(
            own.contains(&Got::Correct(1, Why::Speed)),
            t == 3,
            "tick {t}"
        );
    }
    assert_eq!(
        far_moves, 3,
        "the flag change, then one refresh per ten ticks"
    );
    run(vec![Stamped {
        conn: 1,
        nth: 2,
        received_ms: 1100,
        input: Input::Leave,
    }]);
    assert!(rx[0].next_batch(22).contains(&Got::Vanish(1)));
}

#[test]
fn a_client_that_falls_behind_gets_flag_changes_but_no_refreshes_until_it_catches_up() {
    let spawns = vec![spawn(0.0, 0.0), spawn(10.0, 0.0)];
    let mut sim = Sim::new(spawns, Limits::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let (slow, rx) = Outbox::channel();
    let mut rx = Client::new(rx);
    let behind_by = slow.behind_by();
    shared.hold_outbox(0, slow);
    shared.hold_outbox(1, Outbox::channel().0);
    let pool = pool(1);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run((0..2).map(join).collect());
    rx.welcome();
    assert_eq!(rx.next_batch(0), [Got::Appear(1)]);
    behind_by(0);
    let mut moved = Vec::new();
    for t in 1..=30 {
        match t {
            11 => behind_by(11),
            26 => behind_by(1),
            _ => {}
        }
        let standing = (20..25).contains(&t);
        let flags = if standing { 0 } else { flags::FORWARD };
        let strides = if t < 20 { t } else { t.max(24) - 5 };
        let x = 10.0 + strides as f32 * 0.3;
        let movement = Movement {
            time: t * 50,
            flags,
            pos: [x, 0.0, 0.0],
            ..Movement::default()
        };
        run(vec![claim(1, t, t * 50, 0, movement)]);
        if moves_of(&rx.next_batch(t), 1) > 0 {
            moved.push(t);
        }
    }
    let caught_up: Vec<u32> = (1..=10).chain([20, 25]).chain(26..=30).collect();
    assert_eq!(
        moved, caught_up,
        "ten ticks behind it gets only the stop and the start"
    );
}

#[test]
fn a_client_the_server_gives_up_on_leaves_at_the_next_tick() {
    let spawns = vec![spawn(0.0, 0.0), spawn(10.0, 0.0)];
    let mut sim = Sim::new(spawns, Limits::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let (outbox, rx) = Outbox::channel();
    let mut ada = Client::new(rx);
    shared.hold_outbox(0, outbox);
    let (outbox, mut bo) = Outbox::channel();
    let behind_by = outbox.behind_by();
    shared.hold_outbox(1, outbox);
    let pool = pool(1);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run((0..2).map(join).collect());
    ada.welcome();
    assert_eq!(ada.next_batch(0), [Got::Appear(1)]);
    behind_by(View::default().kick_ticks + 1);
    assert_eq!(run(Vec::new()).built.kicked, 1);
    assert_eq!(ada.next_batch(1), []);
    let hung_up = shared.take_inputs();
    assert!(
        matches!(
            hung_up[..],
            [Stamped {
                conn: 1,
                input: Input::Leave,
                ..
            }]
        ),
        "{hung_up:?}"
    );
    run(hung_up);
    assert_eq!(ada.next_batch(2), [Got::Vanish(1)]);
    while bo.try_recv().is_ok() {}
    assert!(bo.is_closed(), "the server still holds Bo's outbox");
}

#[test]
fn a_body_put_out_of_reach_leaves_at_once_and_comes_where_it_lands_by_the_recheck() {
    let unchecked = Limits {
        check: false,
        ..Limits::default()
    };
    put_out_of_reach(unchecked, join(0), |time, to| claim(0, 1, time, 0, to));
}

#[test]
fn a_host_that_lands_out_of_reach_leaves_its_guests_at_once_and_comes_where_it_lands() {
    put_out_of_reach(Limits::default(), host_join(0), |time, to| {
        teleport(0, 1, time, to)
    });
}

fn put_out_of_reach(limits: Limits, first: Stamped, put: impl Fn(u32, Movement) -> Stamped) {
    let spawns = vec![
        spawn(0.0, 0.0),
        spawn(-10.0, 0.0),
        spawn(0.0, 30.0),
        spawn(400.0, 5.0),
        spawn(395.0, -5.0),
    ];
    let mut sim = Sim::new(spawns, limits, View::default(), 0, 50);
    let shared = Shared::new();
    let mut clients: Vec<Client> = (0..5)
        .map(|conn| {
            let (outbox, rx) = Outbox::channel();
            shared.hold_outbox(conn, outbox);
            Client::new(rx)
        })
        .collect();
    let pool = pool(2);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run([first].into_iter().chain((1..5).map(join)).collect());
    for c in &mut clients {
        c.welcome();
        c.next_batch(0);
    }
    let (put_at, stepped_at) = (8, 14);
    let mut got: Vec<Vec<Vec<Got>>> = (0..5).map(|_| Vec::new()).collect();
    for t in 1..=stepped_at {
        let time = t * 50;
        let inputs = match t {
            _ if t == put_at => {
                let standing = Movement {
                    time,
                    pos: [400.0, 0.0, 0.0],
                    ..Movement::default()
                };
                vec![put(time, standing)]
            }
            _ if t == stepped_at => vec![claim(0, 2, time, 0, running(time, [401.0, 0.0, 0.0]))],
            _ => Vec::new(),
        };
        let st = run(inputs);
        assert_eq!(st.refused.iter().sum::<u32>(), 0, "tick {t}");
        for (o, c) in clients.iter_mut().enumerate().skip(1) {
            got[o].push(c.next_batch(t));
        }
    }
    let at = |o: usize, t: u32| &got[o][t as usize - 1];
    for o in [1, 2] {
        for t in 1..=stepped_at {
            let want: &[Got] = if t == put_at { &[Got::Vanish(0)] } else { &[] };
            assert_eq!(at(o, t), want, "observer {o}, tick {t}");
        }
    }
    let every = View::default().aoi_every;
    for o in [3, 4] {
        let came = (1..=stepped_at).find(|&t| !at(o, t).is_empty());
        assert!(
            came.is_some_and(|t| (put_at..put_at + every).contains(&t)),
            "observer {o} was first shown it at tick {came:?}"
        );
        for t in 1..=stepped_at {
            let want: &[Got] = match t {
                _ if Some(t) == came => &[Got::Appear(0)],
                _ if t == stepped_at => &[Got::Move(0)],
                _ => &[],
            };
            assert_eq!(at(o, t), want, "observer {o}, tick {t}");
        }
    }
}

#[test]
fn a_body_put_out_of_reach_overhead_leaves_at_once_and_is_not_brought_back() {
    let limits = Limits {
        check: false,
        ..Limits::default()
    };
    let spawns = vec![spawn(0.0, 0.0), spawn(10.0, 0.0)];
    let mut sim = Sim::new(spawns, limits, View::default(), 0, 50);
    let shared = Shared::new();
    let (outbox, rx) = Outbox::channel();
    shared.hold_outbox(1, outbox);
    let mut watcher = Client::new(rx);
    let pool = pool(1);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run(vec![join(0), join(1)]);
    watcher.welcome();
    assert_eq!(watcher.next_batch(0), [Got::Appear(0)]);
    let put_at = 3;
    for t in 1..=12 {
        let inputs = if t == put_at {
            let overhead = Movement {
                time: t * 50,
                pos: [0.0, 0.0, 1000.0],
                ..Movement::default()
            };
            vec![claim(0, 1, t * 50, 0, overhead)]
        } else {
            Vec::new()
        };
        run(inputs);
        let want: &[Got] = if t == put_at { &[Got::Vanish(0)] } else { &[] };
        assert_eq!(watcher.next_batch(t), want, "tick {t}");
    }
}

#[test]
fn a_recheck_lets_go_of_the_far_and_brings_in_the_near_on_freed_slots() {
    let xs = [0.0, 150.0, 20.0, 40.0, 60.0, 80.0, 160.0];
    let spawns = xs.iter().map(|&x| spawn(x, 0.0)).collect();
    let limits = Limits {
        check: false,
        ..Limits::default()
    };
    let mut sim = Sim::new(spawns, limits, View::default(), 0, 50);
    let shared = Shared::new();
    let (outbox, rx) = Outbox::channel();
    shared.hold_outbox(0, outbox);
    let mut client = Client::new(rx);
    let pool = pool(2);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run((0..xs.len() as u32).map(join).collect());
    client.welcome();
    assert_eq!(
        client.next_batch(0),
        [
            Got::Appear(2),
            Got::Appear(3),
            Got::Appear(4),
            Got::Appear(5)
        ]
    );
    for t in 1..=10u32 {
        let time = t * 50;
        let inputs = if t == 1 {
            vec![
                claim(2, 1, time, 0, running(time, [200.0, 0.0, 0.0])),
                claim(1, 1, time, 0, running(time, [90.0, 0.0, 0.0])),
            ]
        } else {
            Vec::new()
        };
        run(inputs);
        client.next_batch(t);
    }
    let mut viewed: Vec<u32> = client.slots.values().copied().collect();
    viewed.sort_unstable();
    assert_eq!(
        viewed,
        [1, 3, 4, 5],
        "the one that came, and all that stayed"
    );
    assert!(
        client.slots.keys().all(|&slot| slot < 4),
        "the one that came took the slot of the one that left: {:?}",
        client.slots
    );
}

#[test]
fn the_host_is_put_where_it_asks_and_a_guest_that_asks_is_put_back_and_told_why() {
    let spawns = vec![spawn(0.0, 0.0), spawn(10.0, 0.0)];
    let mut sim = Sim::new(spawns, Limits::default(), View::default(), 0, 50);
    let shared = Shared::new();
    let mut clients: Vec<Client> = (0..2)
        .map(|conn| {
            let (outbox, rx) = Outbox::channel();
            shared.hold_outbox(conn, outbox);
            Client::new(rx)
        })
        .collect();
    let pool = pool(1);
    let mut run = |inputs: Vec<Stamped>| {
        sim.tick(
            &pool,
            &inputs,
            InputOrder::Canonical,
            Batches::Send(&shared),
        )
    };
    run(vec![host_join(0), join(1)]);
    for c in &mut clients {
        c.welcome();
        c.next_batch(0);
    }
    let far = |time: u32, x: f32| Movement {
        time,
        pos: [x, 0.0, 0.0],
        ..Movement::default()
    };
    let asked = run(vec![
        teleport(0, 1, 1000, far(1000, 500.0)),
        teleport(1, 1, 1000, far(1000, 510.0)),
    ]);
    assert_eq!(
        asked.refused[Why::Teleport as usize],
        1,
        "{:?}",
        asked.refused
    );
    let answer = |got: Vec<Got>| {
        let answers = got
            .into_iter()
            .filter(|g| matches!(g, Got::Correct(..) | Got::Granted(_)));
        answers.collect::<Vec<_>>()
    };
    assert_eq!(
        answer(clients[0].next_batch(1)),
        [Got::Granted([500.0, 0.0, 0.0])]
    );
    assert_eq!(
        answer(clients[1].next_batch(1)),
        [Got::Correct(1, Why::Teleport)]
    );
    let after = run(vec![
        claim(0, 2, 1500, 0, running(1500, [503.0, 0.0, 0.0])),
        claim(1, 2, 1500, 0, running(1500, [513.0, 0.0, 0.0])),
        claim(1, 3, 1500, 1, running(1500, [13.0, 0.0, 0.0])),
    ]);
    assert_eq!(
        (after.claims, after.refused.iter().sum::<u32>(), after.stale),
        (3, 0, 1),
        "the host runs on from where it landed, and the guest from where it was put back"
    );
}
