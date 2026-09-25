use std::collections::HashMap;

use game::{Delivery, Game, Id, Kind, Letter, Out, World};
use protocol::{
    Appearance, Claim, Hello, LEN_BYTES, Movement, Record, ServerMessage, VERSION, Why, flags,
};
use server::{Config, Input, InputOrder, Link, Spawn, Stamped, Stepper};

const ROOT: u32 = 1;
const HOME: u32 = 2;

struct Tag;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Called {
    Root,
    Home,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Runner {
    calls: u32,
}

game::knobs! {
    struct Knobs {
        unused: u32,
    }
}

impl Game for Tag {
    const NAME: &'static str = "tag";
    const KNOBS: &'static str = "unused = 0\n";
    type Knobs = Knobs;
    type Msg = Called;
    type Player = Runner;

    fn join(_: Id, _: &World<'_, Self>) -> Runner {
        Runner { calls: 0 }
    }

    fn action(number: u32) -> Option<Called> {
        match number {
            ROOT => Some(Called::Root),
            HOME => Some(Called::Home),
            _ => None,
        }
    }
}

impl Kind<Tag> for Runner {
    type Sent = u32;
    type Saved = u32;

    fn sent(&self) -> u32 {
        self.calls
    }

    fn saved(&self) -> u32 {
        self.calls
    }

    fn apply(
        id: Id,
        me: &mut Self,
        mail: &[Letter<Called>],
        w: &World<'_, Tag>,
        out: &mut Out<Tag>,
    ) {
        for letter in mail {
            me.calls += 1;
            match letter.msg {
                Called::Root => out.root(true),
                Called::Home => {
                    if let Some(spawn) = w.spawn(id) {
                        out.place(spawn);
                    }
                    out.root(false);
                }
            }
        }
    }
}

struct Client {
    link: Link,
    slots: HashMap<u16, u32>,
}

#[derive(Debug, PartialEq)]
enum Got {
    Appear(u32),
    Shown(u32, u32),
    Moved(u32),
    Corrected(u32, Why),
    Placed(u32, bool, [f32; 2], u32),
}

impl Client {
    fn batch(&mut self) -> Vec<Got> {
        let mut got = Vec::new();
        while let Some(frame) = self.link.next_frame() {
            let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&frame[LEN_BYTES..]) else {
                continue;
            };
            for record in batch.map(|r| r.expect("a valid record")) {
                got.push(match record {
                    Record::Appear { slot, id, .. } => {
                        self.slots.insert(slot, id);
                        Got::Appear(id)
                    }
                    Record::Game { slot, state } => {
                        let calls = u32::from_le_bytes(state.try_into().expect("four bytes"));
                        Got::Shown(self.slots[&slot], calls)
                    }
                    Record::Move { slot, .. } | Record::State { slot, .. } => {
                        Got::Moved(self.slots[&slot])
                    }
                    Record::Correct { seq, why, .. } => Got::Corrected(seq, why),
                    Record::Place {
                        seq,
                        rooted,
                        movement,
                    } => Got::Placed(
                        seq,
                        rooted,
                        [movement.pos[0], movement.pos[1]],
                        movement.flags,
                    ),
                    Record::Turn { .. }
                    | Record::Vanish { .. }
                    | Record::Granted { .. }
                    | Record::Show { .. } => {
                        continue;
                    }
                });
            }
        }
        got
    }
}

fn input(conn: u32, nth: u32, input: Input) -> Stamped {
    Stamped {
        conn,
        nth,
        received_ms: nth * 1000,
        input,
    }
}

fn claim(conn: u32, nth: u32, ack: u32, x: f32, facing: f32) -> Stamped {
    let movement = Movement {
        time: nth * 1000,
        flags: flags::FORWARD,
        pos: [x, 0.0, 0.0],
        facing,
        ..Movement::default()
    };
    input(conn, nth, Input::Claim(Claim { ack, movement }))
}

#[test]
fn a_game_roots_and_places_a_body_and_its_observers_are_shown_its_state() {
    let spawns = [0.0, 10.0].map(|x| Spawn {
        pos: [x, 0.0, 0.0],
        facing: 0.0,
    });
    let cfg = Config {
        tick_threads: 2,
        spawns: spawns.to_vec(),
        game: Some(game::load::<Tag>(None, &[], 1).expect("a game")),
        ..Config::default()
    };
    let mut stepper =
        Stepper::new(&cfg, InputOrder::Canonical, Delivery::Canonical).expect("a stepper");
    let mut clients: Vec<Client> = (0..2)
        .map(|conn| Client {
            link: stepper.connect(conn),
            slots: HashMap::new(),
        })
        .collect();
    let hello = |name: &str| Hello {
        version: VERSION,
        name: name.into(),
        appearance: Appearance::default(),
    };
    let ticks = [
        vec![
            input(0, 0, Input::Join(hello("A"))),
            input(1, 0, Input::Join(hello("B"))),
        ],
        vec![claim(1, 1, 0, 13.0, 0.0)],
        vec![input(1, 2, Input::Action(ROOT))],
        vec![claim(1, 3, 0, 16.0, 0.0), claim(1, 4, 1, 16.0, 0.0)],
        vec![claim(1, 5, 2, 13.0, 1.0)],
        vec![input(1, 6, Input::Action(HOME))],
        vec![claim(1, 7, 3, 11.0, 0.0)],
    ];
    let mut seen: Vec<(Vec<Got>, Vec<Got>)> = Vec::new();
    let mut hashes = Vec::new();
    for inputs in &ticks {
        hashes.push(stepper.tick(inputs).hash);
        assert_eq!(stepper.saves_differ(), None);
        seen.push((clients[0].batch(), clients[1].batch()));
    }
    assert_eq!(seen[0].0, [Got::Appear(1), Got::Shown(1, 0)]);
    assert_eq!(seen[2].1, [Got::Placed(1, true, [13.0, 0.0], flags::ROOT)]);
    assert!(seen[2].0.contains(&Got::Shown(1, 1)) && seen[2].0.contains(&Got::Moved(1)));
    assert_eq!(
        seen[3].1,
        [Got::Corrected(2, Why::Speed)],
        "a stale claim, then a walk"
    );
    assert_eq!(seen[4].1, [], "a turn where it stands");
    assert_eq!(seen[5].1, [Got::Placed(3, false, [10.0, 0.0], 0)]);
    assert_eq!(seen[6].1, [], "it walks on from its spawn");
    let game = stepper.game().expect("a game");
    assert_eq!(
        game.shown(1).map(<[u8]>::to_vec),
        Some(2u32.to_le_bytes().to_vec())
    );
    assert!(hashes.windows(2).all(|w| w[0] != w[1]));
}
