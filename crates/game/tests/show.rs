use game::{
    Anim, Delivery, Engine, Game, Hosted, Id, Kind, Kinds, Letter, Out, Outcome, Shows, Spot, Turn,
    World,
};

const SWING: u32 = 1;
const LIE_DOWN: u32 = 2;
const GET_UP: u32 = 3;
const HAUNT: u32 = 4;
const SIT_AND_WAVE: u32 = 5;
const MAKE_READY: u32 = 6;
const CALM_DOWN: u32 = 7;
const STRIKE: u32 = 8;
const WHIFF: u32 = 9;
const STRIKE_A_GHOST: u32 = 10;

const ATTACK: Anim = Anim(16);
const WOUND: Anim = Anim(9);
const DEAD: Anim = Anim(6);
const SIT: Anim = Anim(97);
const READY: Anim = Anim(25);

struct Mime;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Player;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Ghost;

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

    fn kinds(kinds: &mut Kinds<Self>) {
        kinds.add::<Ghost>();
    }

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
                SWING => out.play(ATTACK),
                LIE_DOWN => out.hold(Some(DEAD)),
                GET_UP => out.hold(None),
                HAUNT => out.spawn(Ghost),
                SIT_AND_WAVE => {
                    out.hold(Some(SIT));
                    out.play(WOUND);
                    out.hold(Some(DEAD));
                }
                MAKE_READY => out.idle(Some(READY)),
                CALM_DOWN => out.idle(None),
                STRIKE => {
                    out.play(ATTACK);
                    out.attack(Some(Id::player(1)), Outcome::Hit);
                }
                WHIFF => {
                    out.play(ATTACK);
                    out.attack(None, Outcome::Miss);
                }
                STRIKE_A_GHOST => out.attack(Some(Id { kind: 1, n: 0 }), Outcome::Crit),
                _ => {}
            }
        }
    }
}

impl Kind<Mime> for Ghost {
    type Sent = ();
    type Saved = ();

    fn sent(&self) {}

    fn saved(&self) {}

    fn step(_: Id, _: &mut Self, _: &World<'_, Mime>, out: &mut Out<Mime>) {
        out.play(ATTACK);
        out.hold(Some(DEAD));
    }
}

fn engine() -> Engine<Mime> {
    Engine::new(Knobs { unused: 0 }, 1, 50, Delivery::Canonical)
}

fn tick(e: &mut Engine<Mime>, tick: u32, actions: &[(u32, u32)]) -> Shows {
    let bodies = [Some(Spot::default()); 3];
    let joined: Vec<(u32, Spot)> = (0..3).map(|n| (n, Spot::default())).collect();
    e.tick(&Turn {
        tick,
        joined: if tick == 0 { &joined } else { &[] },
        restored: &[],
        bodies: &bodies,
        actions,
        cpu_ns: || 0,
    });
    e.shows().clone()
}

#[test]
fn a_body_plays_each_animation_once_and_a_pose_is_told_only_as_it_changes() {
    let mut e = engine();
    assert_eq!(tick(&mut e, 0, &[]), Shows::default());
    let shows = tick(
        &mut e,
        1,
        &[(0, SWING), (1, LIE_DOWN), (2, SWING), (0, SWING)],
    );
    assert_eq!(shows.played, [(0, ATTACK), (0, ATTACK), (2, ATTACK)]);
    assert_eq!(shows.held, [(1, Some(DEAD))]);
    assert_eq!((e.held(0), e.held(1)), (None, Some(DEAD)));
    let again = tick(&mut e, 2, &[(1, LIE_DOWN)]);
    assert_eq!(again, Shows::default(), "the pose it already holds");
    let up = tick(&mut e, 3, &[(1, GET_UP), (0, LIE_DOWN), (0, GET_UP)]);
    assert_eq!(
        up.held,
        [(1, None)],
        "a pose let go in the tick it was held is no change"
    );
    assert_eq!(e.held(1), None);
    let sat = tick(&mut e, 4, &[(2, SIT_AND_WAVE)]);
    assert_eq!(sat.played, [(2, WOUND)]);
    assert_eq!(sat.held, [(2, Some(DEAD))], "the last pose a rule held");
}

#[test]
fn a_row_with_no_body_shows_nothing_and_a_held_pose_is_part_of_the_world() {
    let mut e = engine();
    tick(&mut e, 0, &[]);
    tick(&mut e, 1, &[(0, HAUNT)]);
    let haunted = tick(&mut e, 2, &[]);
    assert_eq!(haunted, Shows::default(), "a ghost has no body to show");
    let mut lying = engine();
    tick(&mut lying, 0, &[]);
    tick(&mut lying, 1, &[(0, HAUNT)]);
    tick(&mut lying, 2, &[(1, LIE_DOWN)]);
    assert_ne!(e.hash(), lying.hash());
    tick(&mut lying, 3, &[(1, GET_UP)]);
    tick(&mut e, 3, &[]);
    assert_eq!(
        e.hash(),
        lying.hash(),
        "and once let go, the world is as before"
    );
}

#[test]
fn a_body_idles_until_it_stops_told_as_it_changes_and_apart_from_its_pose() {
    let mut e = engine();
    tick(&mut e, 0, &[]);
    let before = e.hash();
    let ready = tick(
        &mut e,
        1,
        &[(0, MAKE_READY), (1, LIE_DOWN), (1, MAKE_READY)],
    );
    assert_eq!(ready.idled, [(0, Some(READY)), (1, Some(READY))]);
    assert_eq!(ready.held, [(1, Some(DEAD))]);
    assert_eq!(
        (e.idling(0), e.idling(1), e.held(0), e.held(1)),
        (Some(READY), Some(READY), None, Some(DEAD))
    );
    assert_ne!(e.hash(), before, "an idle is part of the world");
    let again = tick(
        &mut e,
        2,
        &[(0, MAKE_READY), (2, MAKE_READY), (2, CALM_DOWN)],
    );
    assert_eq!(
        again,
        Shows::default(),
        "the idle it already has, and one stopped in the tick it began"
    );
    let calm = tick(&mut e, 3, &[(0, CALM_DOWN), (1, CALM_DOWN), (1, GET_UP)]);
    assert_eq!((calm.idled.len(), calm.held.len()), (2, 1));
    assert_eq!(
        e.hash(),
        before,
        "stopped and let go, the world is as it was"
    );
}

#[test]
fn an_attack_is_told_by_attacker_beside_its_swing_and_leaves_nothing_in_the_world() {
    let mut e = engine();
    tick(&mut e, 0, &[]);
    let before = e.hash();
    let shows = tick(&mut e, 1, &[(2, WHIFF), (0, STRIKE), (0, STRIKE_A_GHOST)]);
    assert_eq!(
        shows.attacked,
        [
            (0, Some(1), Outcome::Hit),
            (0, None, Outcome::Crit),
            (2, None, Outcome::Miss)
        ],
        "a row that is not a player's has no body to be attacked"
    );
    assert_eq!(shows.played, [(0, ATTACK), (2, ATTACK)]);
    assert_eq!(
        e.hash(),
        before,
        "an attack, like a play, is no part of the world"
    );
    assert_eq!(tick(&mut e, 2, &[]), Shows::default());
}
