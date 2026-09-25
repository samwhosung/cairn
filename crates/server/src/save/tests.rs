use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use game::{Bytes, Columns, Schema};

use super::*;

mod older {
    game::saved! {
        pub struct Score {
            pub kills: u32,
        }
    }
}

mod newer {
    game::saved! {
        pub struct Score {
            pub kills: u32,
            pub streak: u32 = 7,
        }
    }
}

mod plain {
    game::saved! {
        pub struct Score {
            pub kills: u32,
            pub streak: u32,
        }
    }
}

mod real {
    game::saved! {
        pub struct Score {
            pub kills: f32,
        }
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cairn-save-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join(format!("{name}.sqlite"));
    for end in ["", "-wal", "-shm", "-lock"] {
        let mut file = path.as_os_str().to_owned();
        file.push(end);
        let _ = std::fs::remove_file(file);
    }
    path
}

fn opened<S: Columns>(path: &Path) -> Result<Opened, String> {
    open(path, Some(("tally", &[Schema::of::<S>()])))
}

fn save(opened: Opened, batch: Batch) {
    let writer = Writer::start(opened, Saving::Held).expect("a writer");
    writer.save(batch);
    writer.finish().expect("its changes durable");
}

fn saved<S: Columns>(o: &Opened, name: &str) -> Option<S> {
    let player = o.players.iter().find(|p| p.name == name)?;
    S::from_bytes(player.saved.as_deref()?)
}

fn ada_kills(path: &Path, kills: u32) {
    let o = opened::<older::Score>(path).expect("a new world");
    let row = older::Score { kills }.to_bytes();
    save(
        o,
        Batch {
            tick: 5,
            joined: vec![(0, "Ada".into())],
            rows: vec![(0, row)],
            places: vec![(0, Place::default())],
        },
    );
}

#[test]
fn a_world_keeps_what_a_tick_saves_and_starts_from_it() {
    let path = scratch("keeps");
    ada_kills(&path, 3);
    let o = opened::<older::Score>(&path).expect("the world again");
    assert_eq!(
        saved::<older::Score>(&o, "Ada"),
        Some(older::Score { kills: 3 })
    );
    assert_eq!(o.players[0].place, Some(Place::default()));
    drop(o);
    let world = read(
        &path,
        &["SELECT key, value FROM world ORDER BY key".into()],
        Duration::from_secs(5),
    );
    assert_eq!(
        world.as_deref(),
        Ok("key\tvalue\ngame\ttally\nruns\t2\ntick\tNULL\n")
    );
}

#[test]
fn an_older_file_gains_a_new_fields_declared_default() {
    let path = scratch("older");
    ada_kills(&path, 3);
    let o = opened::<newer::Score>(&path).expect("migrates");
    let got = saved::<newer::Score>(&o, "Ada");
    assert_eq!(
        got,
        Some(newer::Score {
            kills: 3,
            streak: 7
        })
    );
    drop(o);
    let control = scratch("older-control");
    ada_kills(&control, 3);
    let o = opened::<plain::Score>(&control).expect("migrates");
    let got = saved::<plain::Score>(&o, "Ada").map(|s| s.streak);
    assert_eq!(
        got,
        Some(0),
        "a field declared with no default takes its type's"
    );
}

#[test]
fn a_field_that_changed_what_it_holds_is_refused() {
    let path = scratch("changed");
    ada_kills(&path, 3);
    let refused = opened::<real::Score>(&path).map(drop);
    let said = refused.expect_err("a field that changed type");
    assert!(
        said.contains("`score.kills` holds INTEGER NOT NULL, and tally saves it as REAL NOT NULL"),
        "{said}"
    );
    opened::<older::Score>(&path).expect("the control: the same file opens as it was");
}

#[test]
fn a_newer_file_is_refused_by_the_older_build() {
    let path = scratch("newer");
    ada_kills(&path, 3);
    opened::<older::Score>(&path).expect("the control: the older build opens its own file");
    drop(opened::<newer::Score>(&path).expect("the newer build migrates it"));
    let said = opened::<older::Score>(&path).map(drop).expect_err("newer");
    assert!(
        said.contains("`score.streak` is saved there, and this build of tally saves no such field"),
        "{said}"
    );
}

#[test]
fn a_world_is_its_games_of_its_layout_and_one_servers() {
    let path = scratch("its-own");
    ada_kills(&path, 1);
    let other = open(&path, Some(("other", &[Schema::of::<older::Score>()])));
    let said = other.map(drop).expect_err("another game");
    assert!(
        said.contains("a world of tally, and this server runs other"),
        "{said}"
    );
    let none = open(&path, None).map(drop).expect_err("no game");
    assert!(none.contains("this server runs no game"), "{none}");
    let first = opened::<older::Score>(&path).expect("one server");
    let second = opened::<older::Score>(&path).map(drop).expect_err("two");
    assert!(
        second.contains("another server keeps this world"),
        "{second}"
    );
    drop(first);
    opened::<older::Score>(&path).expect("once the first has let go");
    let conn = rusqlite::Connection::open(&path).expect("the file");
    conn.pragma_update(None, "user_version", 2)
        .expect("a later layout");
    drop(conn);
    let said = opened::<older::Score>(&path).map(drop).expect_err("later");
    assert!(
        said.contains("a world of layout 2, from a later build"),
        "{said}"
    );
    let stranger = scratch("stranger");
    let conn = rusqlite::Connection::open(&stranger).expect("a file");
    conn.execute_batch("CREATE TABLE notes (n INTEGER)")
        .expect("a table");
    drop(conn);
    let said = opened::<older::Score>(&stranger)
        .map(drop)
        .expect_err("not a world");
    assert!(said.contains("holds tables, and is not a world"), "{said}");
}

#[test]
fn a_read_never_writes_and_gives_up_after_its_timeout() {
    let path = scratch("read");
    ada_kills(&path, 4);
    let short = Duration::from_millis(100);
    let rows = read(
        &path,
        &["SELECT name, kills FROM player JOIN score ON player = id".into()],
        short,
    );
    assert_eq!(rows.as_deref(), Ok("name\tkills\nAda\t4\n"));
    let write = read(&path, &["UPDATE score SET kills = 9".into()], short);
    assert!(
        write.is_err_and(|e| e.contains("readonly")),
        "a read writes nothing"
    );
    let long = "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 3000000) \
                SELECT count(*) FROM c";
    let stopped = read(&path, &[long.into()], short).expect_err("stopped");
    assert!(stopped.contains("stopped after 0.1 s"), "{stopped}");
    let started = Instant::now();
    let control = read(&path, &[long.into()], Duration::from_secs(60));
    assert_eq!(control.as_deref(), Ok("count(*)\n3000000\n"));
    assert!(
        started.elapsed() > short,
        "the control runs past the timeout"
    );
}
