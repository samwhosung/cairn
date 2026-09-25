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
                        for record in batch.flatten() {
                            if let Record::Place { seq, .. } | Record::Correct { seq, .. } = record
                            {
                                self.told[conn].last_seq = seq;
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
        let conn = self.links.len() as u32;
        self.links.push(self.stepper.connect(conn));
        self.nth.push(0);
        self.told.push(Told::default());
        let hello = Hello {
            version: VERSION,
            name: name.into(),
            appearance: Appearance::default(),
        };
        let join = self.input(conn, Input::Join(hello));
        self.tick(&[join]);
        (conn, self.told[conn as usize].welcome)
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
fn a_join_under_the_name_of_a_player_in_the_world_is_refused_until_it_leaves() {
    let mut run = Run::new(&config(Some(scratch("clash"))));
    let (a, first) = run.join("Ada");
    let (_, twin) = run.join("Ada");
    assert!(
        first.is_some() && twin.is_none(),
        "no welcome for the second Ada"
    );
    assert_eq!(run.stepper.keeping().len(), 1);
    run.leave(a);
    let (again, welcome) = run.join("Ada");
    assert!(
        welcome.is_some() && run.body(again).is_some(),
        "once the first has left"
    );
    assert_eq!(run.stepper.file_differs(), None);
}
