use std::collections::HashMap;

use game::{Anim, Delivery, Game, Id, Kind, Letter, Out, World};
use protocol::{Appearance, Hello, LEN_BYTES, Record, ServerMessage, Show, VERSION, Whose};
use server::{Config, Input, InputOrder, Link, Spawn, Stamped, Stepper};

const SWING: u32 = 1;
const LIE_DOWN: u32 = 2;
const GET_UP: u32 = 3;
const ATTACK: u16 = 16;
const DEAD: u16 = 6;

struct Mime;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Player;

game::knobs! {
    struct Knobs {
        unused: u32,
    }
}

impl Game for Mime {
    const NAME: &'static str = "mime";
    const KNOBS: &'static str = "unused = 0\n";
    type Knobs = Knobs;
    type Msg = u32;
    type Player = Player;

    fn join(_: Id, _: Option<()>, _: &World<'_, Self>) -> Player {
        Player
    }

    fn action(number: u32) -> Option<u32> {
        Some(number)
    }
}

impl Kind<Mime> for Player {
    type Sent = ();
    type Saved = ();

    fn sent(&self) {}

    fn saved(&self) {}

    fn apply(_: Id, _: &mut Self, mail: &[Letter<u32>], _: &World<'_, Mime>, out: &mut Out<Mime>) {
        for letter in mail {
            match letter.msg {
                SWING => out.play(Anim(ATTACK)),
                LIE_DOWN => out.hold(Some(Anim(DEAD))),
                GET_UP => out.hold(None),
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum On {
    Other(u32),
    Own,
}

struct Client {
    link: Link,
    slots: HashMap<u16, u32>,
}

impl Client {
    fn on(&self, whose: Whose) -> On {
        match whose {
            Whose::Slot(slot) => On::Other(self.slots[&slot]),
            Whose::Own => On::Own,
        }
    }

    fn shown(&mut self) -> Vec<(On, Show)> {
        let mut got = Vec::new();
        while let Some(frame) = self.link.next_frame() {
            let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&frame[LEN_BYTES..]) else {
                continue;
            };
            for record in batch.map(|r| r.expect("a valid record")) {
                match record {
                    Record::Appear { slot, id, .. } => {
                        self.slots.insert(slot, id);
                    }
                    Record::Show { whose, show } => got.push((self.on(whose), show)),
                    _ => {}
                }
            }
        }
        got
    }
}

fn join(conn: u32) -> Stamped {
    act(
        conn,
        Input::Join(Hello {
            version: VERSION,
            name: format!("{conn}"),
            appearance: Appearance::default(),
        }),
    )
}

fn act(conn: u32, input: Input) -> Stamped {
    Stamped {
        conn,
        nth: 0,
        received_ms: 0,
        input,
    }
}

#[test]
fn a_body_is_shown_to_whoever_sees_it_and_its_player_and_a_pose_comes_with_the_appear() {
    let spawns = [0.0, 10.0, 500.0, 5.0].map(|x| Spawn {
        pos: [x, 0.0, 0.0],
        facing: 0.0,
    });
    let cfg = Config {
        tick_threads: 2,
        spawns: spawns.to_vec(),
        game: Some(game::load::<Mime>(None, &[], 1).expect("a game")),
        ..Config::default()
    };
    let mut stepper =
        Stepper::new(&cfg, InputOrder::Canonical, Delivery::Canonical).expect("a stepper");
    let mut clients: Vec<Client> = (0..4)
        .map(|conn| Client {
            link: stepper.connect(conn),
            slots: HashMap::new(),
        })
        .collect();
    let (a, b, c, late) = (0, 1, 2, 3);
    let ticks = [
        vec![join(a), join(b), join(c)],
        vec![act(b, Input::Action(SWING))],
        vec![act(b, Input::Action(LIE_DOWN))],
        vec![join(late)],
        vec![act(b, Input::Action(GET_UP)), act(c, Input::Action(SWING))],
    ];
    let mut seen = Vec::new();
    for inputs in &ticks {
        stepper.tick(inputs);
        seen.push(clients.iter_mut().map(Client::shown).collect::<Vec<_>>());
        for id in 0..4 {
            let played = stepper.played_to(id);
            let told: Vec<(On, Show)> = played
                .iter()
                .map(|&(whose, anim)| (clients[id as usize].on(whose), Show::Play(anim)))
                .collect();
            let plays: Vec<(On, Show)> = seen[seen.len() - 1][id as usize]
                .iter()
                .copied()
                .filter(|(_, s)| matches!(s, Show::Play(_)))
                .collect();
            assert_eq!(plays, told, "the stepper says what {id} was played");
        }
    }
    let nothing: Vec<(On, Show)> = Vec::new();
    let played = |on| vec![(on, Show::Play(ATTACK))];
    let held = |on, pose| vec![(on, Show::Hold(pose))];
    assert_eq!(
        seen[1],
        [
            played(On::Other(b)),
            played(On::Own),
            nothing.clone(),
            nothing.clone()
        ]
    );
    let lying = Some(DEAD);
    assert_eq!(
        seen[2],
        [
            held(On::Other(b), lying),
            held(On::Own, lying),
            nothing.clone(),
            nothing.clone()
        ]
    );
    assert_eq!(
        seen[3][late as usize],
        held(On::Other(b), lying),
        "the late one sees b lie as it appears"
    );
    assert_eq!(
        seen[3][a as usize], nothing,
        "and no one else is told again"
    );
    assert_eq!(
        seen[4],
        [
            held(On::Other(b), None),
            held(On::Own, None),
            played(On::Own),
            held(On::Other(b), None)
        ],
        "c, out of everyone's view, is shown only to itself"
    );
    assert_eq!(stepper.pose_of(b), None);
}
