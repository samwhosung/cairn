use std::collections::HashMap;

use game::{Anim, Delivery, Game, Id, Kind, Letter, Out, World};
use protocol::{
    Appearance, Hello, LEN_BYTES, Outcome, Record, ServerMessage, Show, VERSION, Whose,
};
use server::{Config, Input, InputOrder, Link, Spawn, Stamped, Stepper};

const SWING: u32 = 1;
const LIE_DOWN: u32 = 2;
const GET_UP: u32 = 3;
const MAKE_READY: u32 = 4;
const CALM_DOWN: u32 = 5;
const STRIKE_THE_NEAR_ONE: u32 = 6;
const STRIKE_THE_FAR_ONE: u32 = 7;
const ATTACK: u16 = 16;
const DEAD: u16 = 6;
const READY: u16 = 25;

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
                MAKE_READY => out.idle(Some(Anim(READY))),
                CALM_DOWN => out.idle(None),
                STRIKE_THE_NEAR_ONE => {
                    out.play(Anim(ATTACK));
                    out.attack(Some(Id::player(1)), game::Outcome::Crit);
                }
                STRIKE_THE_FAR_ONE => out.attack(Some(Id::player(2)), game::Outcome::Miss),
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

    fn attacks_in(&self, shown: &[(On, Show)]) -> Vec<(On, Option<On>, Outcome)> {
        shown
            .iter()
            .filter_map(|&(on, show)| match show {
                Show::Attack { target, outcome } => Some((on, target.map(|t| self.on(t)), outcome)),
                _ => None,
            })
            .collect()
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

fn four_clients() -> (Stepper, Vec<Client>) {
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
    let stepper =
        Stepper::new(&cfg, InputOrder::Canonical, Delivery::Canonical).expect("a stepper");
    let clients: Vec<Client> = (0..4)
        .map(|conn| Client {
            link: stepper.connect(conn),
            slots: HashMap::new(),
        })
        .collect();
    (stepper, clients)
}

#[test]
fn a_body_is_shown_to_whoever_sees_it_and_its_player_and_a_pose_comes_with_the_appear() {
    let (mut stepper, mut clients) = four_clients();
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

#[test]
fn what_a_body_idles_in_is_told_to_whoever_sees_it_and_its_player_and_comes_with_the_appear() {
    let (mut stepper, mut clients) = four_clients();
    let (a, b, late) = (0, 1, 3);
    let ticks = [
        vec![join(a), join(b), join(2)],
        vec![act(b, Input::Action(MAKE_READY))],
        vec![join(late)],
        vec![act(b, Input::Action(CALM_DOWN))],
    ];
    let mut seen = Vec::new();
    for inputs in &ticks {
        stepper.tick(inputs);
        seen.push(clients.iter_mut().map(Client::shown).collect::<Vec<_>>());
        if seen.len() == 3 {
            let view = stepper.in_view(late).expect("a view");
            let b_there = view.iter().find(|v| v.id == b).expect("b in view");
            assert_eq!(b_there.idle, Some(READY), "the stepper says b idles ready");
            assert_eq!(stepper.idle_of(b), Some(READY));
        }
    }
    let nothing: Vec<(On, Show)> = Vec::new();
    let idled = |on, anim| vec![(on, Show::Idle(anim))];
    assert_eq!(
        seen[1],
        [
            idled(On::Other(b), Some(READY)),
            idled(On::Own, Some(READY)),
            nothing.clone(),
            nothing.clone()
        ],
        "the one far off is not told"
    );
    assert_eq!(
        seen[2][late as usize],
        idled(On::Other(b), Some(READY)),
        "the late one sees b ready as it appears"
    );
    assert_eq!(
        seen[2][a as usize], nothing,
        "and no one else is told again"
    );
    assert_eq!(
        seen[3],
        [
            idled(On::Other(b), None),
            idled(On::Own, None),
            nothing.clone(),
            idled(On::Other(b), None)
        ]
    );
    assert_eq!(stepper.idle_of(b), None);
}

#[test]
fn an_attack_is_told_to_whoever_sees_the_attacker_with_the_one_attacked_as_each_sees_it() {
    let (mut stepper, mut clients) = four_clients();
    let (a, b, c, late) = (0, 1, 2, 3);
    stepper.tick(&[join(a), join(b), join(c)]);
    for client in &mut clients {
        client.shown();
    }
    stepper.tick(&[
        act(a, Input::Action(STRIKE_THE_NEAR_ONE)),
        act(a, Input::Action(STRIKE_THE_FAR_ONE)),
    ]);
    let told: Vec<Vec<(On, Option<On>, Outcome)>> = clients
        .iter_mut()
        .map(|client| {
            let shown = client.shown();
            client.attacks_in(&shown)
        })
        .collect();
    assert_eq!(
        told[a as usize],
        [
            (On::Own, Some(On::Other(b)), Outcome::Crit),
            (On::Own, None, Outcome::Miss)
        ],
        "its player is told both, and c is out of its view"
    );
    assert_eq!(
        told[b as usize],
        [
            (On::Other(a), Some(On::Own), Outcome::Crit),
            (On::Other(a), None, Outcome::Miss)
        ]
    );
    assert_eq!(
        told[c as usize],
        [],
        "c, far off, is told nothing, though a attacked it"
    );
    assert_eq!(told[late as usize], []);
    let stepped: Vec<(On, Option<On>, Outcome)> = stepper
        .attacks_to(b)
        .into_iter()
        .map(|(w, t, o)| {
            (
                clients[b as usize].on(w),
                t.map(|t| clients[b as usize].on(t)),
                o,
            )
        })
        .collect();
    assert_eq!(
        stepped, told[b as usize],
        "the stepper says what b was told"
    );
    assert_eq!(stepper.attacks_to(c), []);
}
