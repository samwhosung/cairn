//! A small game played through the rule API alone: players that kindle, and sparks they spawn
//! that warm a player each tick until they burn out.

use std::collections::BTreeMap;

use game::{
    Bytes, Delivery, Engine, Game, Hosted, Id, Kind, Kinds, Letter, Out, Saves, Spot, Turn, World,
};

struct Embers;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Msg {
    Kindle,
    Warm(u32),
    Thanks,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Ember {
    heat: u32,
    pending: u32,
    kindled: u32,
    thanked: Option<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Spark {
    owner: Id,
    left: u32,
}

game::knobs! {
    struct Knobs {
        sparks: u32,
        life: u32,
    }
}

impl Game for Embers {
    const NAME: &'static str = "embers";
    const KNOBS: &'static str = "sparks = 3\nlife = 4\n";
    type Knobs = Knobs;
    type Msg = Msg;
    type Player = Ember;

    fn kinds(kinds: &mut Kinds<Self>) {
        kinds.add::<Spark>();
    }

    fn join(_: Id, _: &World<'_, Self>) -> Ember {
        Ember {
            heat: 0,
            pending: 0,
            kindled: 0,
            thanked: None,
        }
    }

    fn action(number: u32) -> Option<Msg> {
        (number == 1).then_some(Msg::Kindle)
    }
}

impl Kind<Embers> for Ember {
    type Sent = u32;
    type Saved = (u32, Option<Id>);

    fn sent(&self) -> u32 {
        self.heat
    }

    fn saved(&self) -> (u32, Option<Id>) {
        (self.kindled, self.thanked)
    }

    fn step(id: Id, me: &mut Self, w: &World<'_, Embers>, out: &mut Out<Embers>) {
        out.count("ember steps", 1);
        for _ in 0..std::mem::take(&mut me.pending) {
            out.spawn(Spark {
                owner: id,
                left: w.knobs().life,
            });
        }
    }

    fn apply(
        _: Id,
        me: &mut Self,
        mail: &[Letter<Msg>],
        w: &World<'_, Embers>,
        out: &mut Out<Embers>,
    ) {
        let mut warmed = false;
        for letter in mail {
            match letter.msg {
                Msg::Kindle => {
                    me.kindled += 1;
                    me.pending += w.knobs().sparks;
                    out.wake_at(w.tick());
                }
                Msg::Warm(by) => {
                    me.heat += by;
                    if !warmed {
                        warmed = true;
                        me.thanked = Some(letter.from);
                        out.send(letter.from, Msg::Thanks);
                    }
                }
                Msg::Thanks => {}
            }
        }
    }
}

impl Kind<Embers> for Spark {
    type Sent = u32;
    type Saved = Id;

    fn sent(&self) -> u32 {
        self.left
    }

    fn saved(&self) -> Id {
        self.owner
    }

    fn step(id: Id, me: &mut Self, w: &World<'_, Embers>, out: &mut Out<Embers>) {
        let players = w.table::<Ember>().map_or(0, game::Table::len) as u32;
        let to = w.range(id, 0, 0, players - 1);
        out.send(Id::player(to), Msg::Warm(1 + me.owner.n % 3));
        me.left -= 1;
        if me.left == 0 {
            out.despawn();
        } else {
            out.wake_at(w.tick() + 1);
        }
    }

    fn apply(
        _: Id,
        me: &mut Self,
        mail: &[Letter<Msg>],
        _: &World<'_, Embers>,
        _: &mut Out<Embers>,
    ) {
        me.left += mail.len() as u32;
    }
}

const PLAYERS: u32 = 40;

fn engine(delivery: Delivery) -> Engine<Embers> {
    let lines = game::lines(Embers::KNOBS, "embers").expect("lines");
    let knobs = <Knobs as game::Knobs>::read(&lines, "embers", &[]).expect("knobs");
    Engine::new(knobs, 11, 50, delivery)
}

/// Runs `ticks` ticks on `threads` threads and hands each tick's engine to `each`.
fn run(threads: usize, delivery: Delivery, ticks: u32, mut each: impl FnMut(&Engine<Embers>)) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("a pool");
    let mut e = engine(delivery);
    let bodies: Vec<Option<Spot>> = (0..PLAYERS)
        .map(|n| {
            Some(Spot {
                pos: [n as f32, 0.0, 0.0],
                facing: 0.0,
            })
        })
        .collect();
    let joined: Vec<(u32, Spot)> = (0..PLAYERS)
        .map(|n| (n, bodies[n as usize].unwrap_or_default()))
        .collect();
    for tick in 0..ticks {
        let actions: Vec<(u32, u32)> = (0..PLAYERS)
            .filter(|p| (tick + p).is_multiple_of(7))
            .map(|p| (p, 1))
            .collect();
        let turn = Turn {
            tick,
            joined: if tick == 0 { &joined } else { &[] },
            bodies: &bodies,
            actions: &actions,
            cpu_ns: || 0,
        };
        pool.install(|| e.tick(&turn));
        each(&e);
    }
}

fn hashes(threads: usize, delivery: Delivery) -> Vec<u64> {
    let mut all = Vec::new();
    run(threads, delivery, 80, |e| all.push(e.hash()));
    all
}

#[test]
fn the_world_is_the_same_on_any_number_of_threads_and_not_in_the_reverse_order() {
    let one = hashes(1, Delivery::Canonical);
    assert_eq!(one, hashes(4, Delivery::Canonical));
    assert_eq!(one, hashes(14, Delivery::Canonical));
    let reversed = hashes(4, Delivery::Reversed);
    let parted = one.iter().zip(&reversed).position(|(a, b)| a != b);
    assert_eq!(parted, Some(1), "the first tick two sparks warm one ember");
    assert_eq!(
        reversed,
        hashes(14, Delivery::Reversed),
        "a fixed wrong order"
    );
}

#[test]
fn spawned_rows_are_numbered_by_who_spawned_them_whatever_the_threads() {
    let sparks = |threads| {
        let mut first = Vec::new();
        run(threads, Delivery::Canonical, 1, |e| {
            let sparks = e.table::<Spark>().expect("a kind");
            first = sparks.iter().map(|(n, s)| (n, s.owner.n)).collect();
        });
        first
    };
    let one = sparks(1);
    let kindled: Vec<u32> = (0..PLAYERS).filter(|p| p.is_multiple_of(7)).collect();
    let want: Vec<(u32, u32)> = kindled
        .iter()
        .flat_map(|&p| [p; 3])
        .enumerate()
        .map(|(n, p)| (n as u32, p))
        .collect();
    assert_eq!(one, want);
    assert_eq!(one, sparks(14));
}

#[test]
fn a_row_with_nothing_due_and_no_letter_is_not_stepped() {
    let mut steps = 0;
    let mut kindles = 0;
    run(4, Delivery::Canonical, 70, |e| {
        steps = e.counts().get("ember steps").copied().unwrap_or(0);
    });
    for tick in 1..70u32 {
        kindles += (0..PLAYERS)
            .filter(|p| (tick + p).is_multiple_of(7))
            .count() as i64;
    }
    assert_eq!(
        steps,
        i64::from(PLAYERS) + kindles,
        "a step each in the first tick, then one a kindle"
    );
}

#[test]
fn a_game_loads_on_its_own_knobs_and_an_overlay_names_its_faults() {
    let over = game::lines("life = 9\n", "over.knobs").expect("lines");
    let loaded = game::load::<Embers>(None, &over, 1).expect("loads");
    assert_eq!(loaded.name(), "embers");
    let bad = game::lines("\nheat = 9\n", "bad.knobs").expect("lines");
    let fault = game::load::<Embers>(None, &bad, 1).expect_err("an unknown key");
    assert_eq!(fault, "bad.knobs:2: `heat` is not a knob of this game");
    let started = loaded.start(50, Delivery::Canonical);
    assert_eq!(started.name(), "embers");
}

#[test]
fn the_record_carries_every_change_of_what_is_sent_and_saved() {
    let mut saves = Saves::default();
    let mut shown: BTreeMap<Id, Vec<u8>> = BTreeMap::new();
    let mut ticks = 0;
    run(4, Delivery::Canonical, 60, |e| {
        let record = e.record();
        saves.take(record);
        for (id, bytes) in &record.shown {
            shown.insert(*id, bytes.clone());
        }
        for id in &record.gone {
            shown.remove(id);
        }
        assert_eq!(
            saves.first_difference(&e.saved()),
            None,
            "tick {}",
            record.tick
        );
        let w = e.world();
        let players = w.table::<Ember>().expect("players").iter();
        let sparks = w.table::<Spark>().expect("sparks").iter();
        let now: BTreeMap<Id, Vec<u8>> = players
            .map(|(n, r)| (Id::player(n), r.sent().to_bytes()))
            .chain(sparks.map(|(n, r)| (Id { kind: 1, n }, r.sent().to_bytes())))
            .collect();
        assert_eq!(shown, now, "tick {}", record.tick);
        ticks += 1;
    });
    assert_eq!(ticks, 60);
    assert!(
        saves.rows().len() > PLAYERS as usize,
        "sparks are saved too"
    );
}
