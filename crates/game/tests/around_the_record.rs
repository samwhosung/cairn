//! A rule that credits a kill straight into the killer's row, around the record, does not compile;
//! the same rule crediting its own row does. Each is built as a crate of its own on this one.

use std::path::Path;
use std::process::{Command, Output};

const GAME: &str = r#"
use game::{Game, Id, Kind, Letter, Out, World};

pub struct Tally;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Won;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Fighter {
    pub kills: u32,
}

game::knobs! {
    pub struct Knobs {
        pub unused: u32,
    }
}

impl Game for Tally {
    const NAME: &'static str = "tally";
    const KNOBS: &'static str = "unused = 0\n";
    type Knobs = Knobs;
    type Msg = Won;
    type Player = Fighter;

    fn join(_: Id, _: &World<'_, Self>) -> Fighter {
        Fighter { kills: 0 }
    }

    fn action(_: u32) -> Option<Won> {
        None
    }
}

impl Kind<Tally> for Fighter {
    type Sent = ();
    type Saved = u32;

    fn sent(&self) {}

    fn saved(&self) -> u32 {
        self.kills
    }

    fn apply(_: Id, me: &mut Self, mail: &[Letter<Won>], w: &World<'_, Tally>, _: &mut Out<Tally>) {
        for letter in mail {
            credit(me, letter.from, w);
        }
    }
}
"#;

fn build(credit: &str) -> Output {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("around-the-record");
    std::fs::create_dir_all(dir.join("src")).expect("a scratch crate");
    let game = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = format!(
        "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [dependencies]\ngame = {{ path = {:?} }}\n\n[workspace]\n",
        game.display().to_string()
    );
    std::fs::write(dir.join("Cargo.toml"), manifest).expect("its manifest");
    let lock = game.join("../../Cargo.lock");
    std::fs::copy(lock, dir.join("Cargo.lock")).expect("the workspace's lock");
    std::fs::write(dir.join("src/lib.rs"), format!("{GAME}\n{credit}\n")).expect("its source");
    Command::new(env!("CARGO"))
        .args(["check", "--offline", "--quiet", "--message-format=short"])
        .arg("--target-dir")
        .arg(dir.join("target"))
        .current_dir(&dir)
        .output()
        .expect("cargo runs")
}

#[test]
fn a_rule_cannot_write_another_row_around_the_record() {
    let own = build("fn credit(me: &mut Fighter, _: Id, _: &World<'_, Tally>) { me.kills += 1; }");
    assert!(
        own.status.success(),
        "{}",
        String::from_utf8_lossy(&own.stderr)
    );
    let around = build(
        "fn credit(_: &mut Fighter, killer: Id, w: &World<'_, Tally>) {\n    \
         w.player(killer).expect(\"a player\").kills += 1;\n}",
    );
    let said = String::from_utf8_lossy(&around.stderr);
    assert!(!around.status.success());
    assert!(
        said.contains("error[E0594]") && said.contains("due to 1 previous error"),
        "{said}"
    );
}
