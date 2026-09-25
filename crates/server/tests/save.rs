use std::path::{Path, PathBuf};

use game::{Delivery, Loaded, Value};
use protocol::{
    Appearance, Claim, Hello, LEN_BYTES, Movement, Record, ServerMessage, VERSION, Welcome, flags,
};
use server::{Config, Input, InputOrder, Link, Spawn, Stamped, Stepper};

const QUICK: &str = "swing_ms = 50\ndamage_min = 60\ndamage_max = 60\nrespawn_s = 1\n";
const MELEE_SWING: u32 = 1;

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("worlds");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join(format!("{name}.sqlite"));
    for end in ["", "-wal", "-shm", "-lock"] {
        let mut file = path.as_os_str().to_owned();
        file.push(end);
        let _ = std::fs::remove_file(file);
    }
    path
}

fn melee() -> Loaded {
    let over = game::KnobsFile::parse(QUICK, "quick.knobs").expect("lines");
    catalog::load("melee", None, &over.lines, 1).expect("melee")
}

fn config(world: Option<PathBuf>) -> Config {
    Config {
        tick_threads: 2,
        spawns: vec![
            Spawn {
                pos: [0.0, 0.0, 0.0],
                facing: 0.0,
            },
            Spawn {
                pos: [3.0, 0.0, 0.0],
                facing: std::f32::consts::PI,
            },
        ],
        game: Some(melee()),
        world,
        ..Config::default()
    }
}

#[derive(Default)]
struct Told {
    welcome: Option<Welcome>,
    last_seq: u32,
    appeared: Vec<u32>,
    vanished: u32,
}

struct Run {
    stepper: Stepper,
    links: Vec<Link>,
    nth: Vec<u32>,
    told: Vec<Told>,
}

impl Run {
    fn new(cfg: &Config) -> Self {
        let stepper =
            Stepper::new(cfg, InputOrder::Canonical, Delivery::Canonical).expect("a world");
        Self {
            stepper,
            links: Vec::new(),
            nth: Vec::new(),
            told: Vec::new(),
        }
    }

    fn tick(&mut self, inputs: &[Stamped]) {
        self.stepper.tick(inputs);
        for conn in 0..self.links.len() {
            while let Some(frame) = self.links[conn].next_frame() {
                match ServerMessage::read(&frame[LEN_BYTES..]) {
                    Ok(ServerMessage::Welcome(w)) => self.told[conn].welcome = Some(w),
                    Ok(ServerMessage::Batch(batch)) => {
                        let told = &mut self.told[conn];
                        for record in batch.flatten() {
                            match record {
                                Record::Place { seq, .. } | Record::Correct { seq, .. } => {
                                    told.last_seq = seq;
                                }
                                Record::Appear { id, .. } => told.appeared.push(id),
                                Record::Vanish { .. } => told.vanished += 1,
                                _ => {}
                            }
                        }
                    }
                    Err(e) => panic!("a frame the server sent: {e}"),
                }
            }
        }
    }

    fn input(&mut self, conn: u32, input: Input) -> Stamped {
        let nth = &mut self.nth[conn as usize];
        *nth += 1;
        Stamped {
            conn,
            nth: *nth,
            received_ms: *nth * 1000,
            input,
        }
    }

    fn join(&mut self, name: &str) -> (u32, Option<Welcome>) {
        self.join_as(name, Input::Join)
    }

    fn join_as(&mut self, name: &str, as_: fn(Hello) -> Input) -> (u32, Option<Welcome>) {
        let conn = self.links.len() as u32;
        self.links.push(self.stepper.connect(conn));
        self.nth.push(0);
        self.told.push(Told::default());
        let hello = Hello {
            version: VERSION,
            name: name.into(),
            appearance: Appearance::default(),
        };
        let join = self.input(conn, as_(hello));
        self.tick(&[join]);
        (conn, self.told[conn as usize].welcome)
    }

    /// The game's health and death of the body `conn` was welcomed to.
    fn shown(&self, conn: u32) -> Option<(u32, bool)> {
        let id = self.told[conn as usize].welcome?.id;
        game::Bytes::from_bytes(self.stepper.game()?.shown(id)?)
    }

    fn body(&self, conn: u32) -> Option<game::Spot> {
        self.stepper.body(self.told[conn as usize].welcome?.id)
    }

    fn swing(&mut self, conn: u32, ticks: u32) {
        for _ in 0..ticks {
            let swing = self.input(conn, Input::Action(MELEE_SWING));
            self.tick(&[swing]);
        }
    }

    fn idle(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.tick(&[]);
        }
    }

    fn walk(&mut self, conn: u32, yd: f32) -> [f32; 3] {
        let i = conn as usize;
        let body = self.body(conn).expect("in the world");
        let to = [body.pos[0], body.pos[1] + yd, body.pos[2]];
        let movement = Movement {
            time: (self.nth[i] + 1) * 1000,
            flags: flags::FORWARD,
            pos: to,
            facing: body.facing,
            ..Movement::default()
        };
        let ack = self.told[i].last_seq;
        let claim = self.input(conn, Input::Claim(Claim { ack, movement }));
        self.tick(&[claim]);
        to
    }

    fn leave(&mut self, conn: u32) {
        let leave = self.input(conn, Input::Leave);
        self.tick(&[leave]);
    }

    fn score(&self, name: &str) -> Option<Vec<Value>> {
        self.stepper.keeping().get(name)?.saved.clone()
    }
}

fn score(kills: i64, deaths: i64) -> Vec<Value> {
    vec![Value::Integer(kills), Value::Integer(deaths)]
}

#[test]
fn a_player_that_leaves_and_comes_back_gets_its_kills_deaths_and_place() {
    let world = scratch("coming-back");
    let mut run = Run::new(&config(Some(world.clone())));
    let (a, _) = run.join("Ada");
    let (b, _) = run.join("Bo");
    run.swing(a, 60);
    run.idle(30);
    let (ada, bo) = (run.score("Ada"), run.score("Bo"));
    assert!(
        bo.as_ref().is_some_and(|s| s[1] != Value::Integer(0)),
        "{bo:?}"
    );
    let stood = run.walk(b, 4.0);
    assert_eq!(
        run.body(b).map(|s| s.pos),
        Some(stood),
        "the walk was taken"
    );
    run.leave(b);
    let (b, back) = run.join("Bo");
    let back = back.map(|w| w.spawn.pos);
    assert_eq!((run.score("Bo"), back), (bo.clone(), Some(stood)));
    assert_eq!(run.stepper.file_differs(), None);
    let (_, fresh) = run.join("Cy");
    assert_eq!(
        run.score("Cy"),
        Some(score(0, 0)),
        "the control: a name new to the world"
    );
    assert_ne!(fresh.map(|w| w.spawn.pos), Some(stood));
    run.leave(b);
    run.stepper.finish().expect("durable");

    let mut run = Run::new(&config(Some(world)));
    run.join("Ada");
    let (_, back) = run.join("Bo");
    assert_eq!(
        (run.score("Ada"), run.score("Bo")),
        (ada.clone(), bo.clone())
    );
    assert_eq!(back.map(|w| w.spawn.pos), Some(stood), "where it left");
    assert_eq!(run.stepper.file_differs(), None);
    run.stepper.finish().expect("durable");

    let mut run = Run::new(&config(None));
    run.join("Ada");
    assert_eq!(
        run.score("Ada"),
        Some(score(0, 0)),
        "the control: a world kept in memory starts empty"
    );
}

#[test]
fn a_join_under_the_name_of_a_player_in_the_world_takes_over_its_body_and_row() {
    let mut run = Run::new(&config(Some(scratch("takeover"))));
    let (a, first) = run.join("Ada");
    let (b, _) = run.join("Bo");
    run.swing(b, 1);
    let stood = run.walk(a, 2.0);
    let (_, cy) = run.join("Cy");
    let first = first.expect("Ada's welcome");
    assert!(
        cy.is_some_and(|w| w.id != first.id) && !run.links[a as usize].closed(),
        "the control: a join under another name gets a body of its own"
    );
    let seen_before = (
        run.told[b as usize].appeared.len(),
        run.told[b as usize].vanished,
    );
    let (again, second) = run.join("Ada");
    let second = second.expect("a welcome for the new session");
    assert_eq!(second.id, first.id, "the same body");
    assert_eq!(
        second.spawn.pos.map(f32::to_bits),
        stood.map(f32::to_bits),
        "where it stands, not a spawn"
    );
    assert!(run.links[a as usize].closed(), "the old session is let go");
    assert_eq!(
        run.shown(again),
        Some((40, false)),
        "the same row: hit once"
    );
    let seen = (
        run.told[b as usize].appeared.len(),
        run.told[b as usize].vanished,
    );
    assert_eq!(seen, seen_before, "Bo sees Ada neither go nor come");
    let moved = run.walk(again, 2.0);
    assert_eq!(
        run.body(again).map(|s| s.pos),
        Some(moved),
        "the new session moves it"
    );
    run.walk(a, 5.0);
    assert_eq!(
        run.body(again).map(|s| s.pos),
        Some(moved),
        "the old one no longer does"
    );
    assert_eq!(run.stepper.file_differs(), None);
}

#[test]
fn a_guest_never_takes_over_the_hosts_body() {
    let mut run = Run::new(&config(None));
    let (host, first) = run.join_as("Ada", Input::HostJoin);
    let (_, guest) = run.join("Ada");
    assert!(first.is_some() && guest.is_none() && !run.links[host as usize].closed());
    let (_, again) = run.join_as("Ada", Input::HostJoin);
    assert_eq!(
        again.map(|w| w.id),
        first.map(|w| w.id),
        "the host takes its own back"
    );
}
