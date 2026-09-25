use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scenarios() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios")
}

fn run(file: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bots"))
        .arg("scenario")
        .arg(file)
        .args(args)
        .output()
        .expect("bots runs")
}

fn said(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn machine_independent(out: &Output) -> String {
    let line = String::from_utf8_lossy(&out.stdout);
    let end = line
        .find(",\"wall_s\"")
        .unwrap_or_else(|| panic!("no verdict: {}", said(out)));
    line[..end].to_string()
}

fn hash(verdict: &str) -> &str {
    let at = verdict.find("\"hash\":\"").expect("a world hash") + 8;
    &verdict[at..at + 16]
}

fn scratch(name: &str, text: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scenarios");
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let file = dir.join(name);
    std::fs::write(&file, text).expect("a scenario written");
    file
}

fn over(base: &str, lines: &str) -> String {
    format!("base = {}\n{lines}", scenarios().join(base).display())
}

#[test]
fn every_scenario_holds_what_it_expects() {
    let install = std::env::var_os("WOW_DATA").is_some();
    let mut files: Vec<PathBuf> = std::fs::read_dir(scenarios())
        .expect("the scenarios")
        .map(|e| e.expect("an entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "scenario"))
        .collect();
    files.sort();
    let mut failed = Vec::new();
    for file in &files {
        let out = run(file, &["--threads", "4"]);
        match out.status.code() {
            Some(0) => {}
            Some(3) if !install => eprintln!("{}: skipped, on the install", file.display()),
            code => failed.push(format!("{}: {code:?}\n{}", file.display(), said(&out))),
        }
    }
    assert!(
        !files.is_empty(),
        "no scenarios in {}",
        scenarios().display()
    );
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[test]
fn a_verdict_is_the_same_on_one_thread_and_on_fourteen_but_not_in_a_racy_order() {
    for name in [
        "flat.scenario",
        "cheat-speed.scenario",
        "flat-run14.scenario",
    ] {
        let file = scratch(name, &over(name, "seconds = 15\n"));
        let one = machine_independent(&run(&file, &["--threads", "1"]));
        let many = machine_independent(&run(&file, &["--threads", "14"]));
        assert_eq!(one, many, "{name}");
        let racy = machine_independent(&run(&file, &["--threads", "14", "--racy"]));
        assert_ne!(
            hash(&one),
            hash(&racy),
            "{name}: applied in a racy order, the world kept"
        );
    }
}

#[test]
fn a_false_expectation_fails_the_run_and_is_named() {
    let short = "seconds = 5\n";
    let wrong = scratch(
        "wrong.scenario",
        &over(
            "flat.scenario",
            &format!("{short}expect walkers.corrections == 1\n"),
        ),
    );
    let out = run(&wrong, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    let named = format!(
        "{}:3: expected walkers.corrections == 1, got 0",
        wrong.display()
    );
    assert_eq!(said(&out).trim(), named);
    let right = scratch(
        "right.scenario",
        &over(
            "flat.scenario",
            &format!("{short}expect walkers.corrections == 0\n"),
        ),
    );
    let out = run(&right, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
}

#[test]
fn a_view_past_what_the_wire_reaches_is_refused_at_its_line_and_one_within_it_runs() {
    for (base, radius, view, reach) in [
        ("flat.scenario", (250, 240), "251.0", "242.0"),
        ("flat-run14.scenario", (230, 226), "231.0", "228.0"),
    ] {
        let (past, within) = radius;
        let past = scratch(
            "past.scenario",
            &over(base, &format!("seconds = 5\nview.radius = {past}\n")),
        );
        let out = run(&past, &[]);
        assert_eq!(out.status.code(), Some(2), "{}", said(&out));
        let named = format!(
            "{}:3: a view of {view} yd, its radius and grey, goes past the {reach} yd",
            past.display()
        );
        assert!(said(&out).starts_with(&named), "{}", said(&out));
        let within = scratch(
            "within.scenario",
            &over(base, &format!("seconds = 5\nview.radius = {within}\n")),
        );
        let out = run(&within, &[]);
        assert_eq!(out.status.code(), Some(0), "{base}: {}", said(&out));
    }
}

#[test]
fn an_unknown_key_is_refused_at_its_line_and_a_known_one_is_taken() {
    let unknown = scratch(
        "unknown.scenario",
        &over("flat.scenario", "seconds = 5\nlimits.speed = 7\n"),
    );
    let out = run(&unknown, &[]);
    assert_eq!(out.status.code(), Some(2));
    let fault = format!(
        "{}:3: `limits.speed` is not a key a scenario sets",
        unknown.display()
    );
    assert_eq!(said(&out).trim(), fault);
    let known = scratch(
        "known.scenario",
        &over("flat.scenario", "seconds = 5\nlimits.run = 7\n"),
    );
    let out = run(&known, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
}

fn melee_on(name: &str, lines: &str) -> PathBuf {
    scratch(
        name,
        &over("melee.scenario", &format!("place = flat\n{lines}")),
    )
}

fn hashes(file: &Path, args: &[&str], out: &str) -> Vec<String> {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("scenarios");
    let at = dir.join(out);
    let path = at.to_str().expect("a path");
    let ran = run(file, &[args, &["--hashes", path]].concat());
    assert!(ran.status.code().is_some_and(|c| c < 2), "{}", said(&ran));
    let text = std::fs::read_to_string(&at).expect("the hashes");
    text.lines().map(str::to_owned).collect()
}

#[test]
fn a_game_is_one_world_on_one_four_and_fourteen_threads_and_parts_from_it_in_reverse() {
    let file = melee_on("melee-threads.scenario", "seconds = 20\n");
    let one = machine_independent(&run(&file, &["--threads", "1"]));
    assert!(one.contains("\"game\":{\"name\":\"melee\""), "{one}");
    for threads in ["4", "14"] {
        let many = machine_independent(&run(&file, &["--threads", threads]));
        assert_eq!(one, many, "{threads} threads");
    }
    let canonical = hashes(&file, &["--threads", "4"], "canonical.hashes");
    let reversed = hashes(&file, &["--threads", "14", "--reversed"], "reversed.hashes");
    let parted = canonical.iter().zip(&reversed).position(|(a, b)| a != b);
    assert_eq!(
        parted,
        Some(82),
        "where two blows first land on one bot and kill it"
    );
    let again = hashes(&file, &["--threads", "4", "--reversed"], "again.hashes");
    assert_eq!(reversed, again, "a fixed wrong order");
}

#[test]
fn a_bot_that_drops_one_state_it_was_sent_is_caught() {
    let whole = machine_independent(&run(
        &melee_on("melee-whole.scenario", "seconds = 5\n"),
        &[],
    ));
    assert!(whole.contains("\"shown_mismatches\":0"), "{whole}");
    let dropping = melee_on(
        "melee-drop.scenario",
        "seconds = 5\nbots.fighters.drop_shown = 1\n",
    );
    let out = run(&dropping, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        said(&out).contains("expected game.shown_mismatches == 0"),
        "{}",
        said(&out)
    );
}

#[test]
fn a_bot_that_drops_one_animation_pose_idle_or_attack_it_was_shown_is_caught() {
    let quick = "seconds = 8\nknobs.damage_min = 60\nknobs.damage_max = 60\n";
    let whole = machine_independent(&run(&melee_on("melee-shows.scenario", quick), &[]));
    assert!(whole.contains("\"shown_mismatches\":0"), "{whole}");
    for drop in ["drop_played", "drop_held", "drop_idled", "drop_attacked"] {
        let dropping = melee_on(
            &format!("melee-{drop}.scenario"),
            &format!("{quick}bots.fighters.{drop} = 1\n"),
        );
        let out = run(&dropping, &[]);
        assert_eq!(out.status.code(), Some(1), "{drop}: {}", said(&out));
        assert!(
            said(&out).contains("expected game.shown_mismatches == 0"),
            "{drop}: {}",
            said(&out)
        );
    }
}

#[test]
fn a_writer_that_drops_one_change_leaves_the_file_apart_from_the_world() {
    let whole = machine_independent(&run(
        &melee_on("melee-kept.scenario", "seconds = 10\n"),
        &[],
    ));
    assert!(whole.contains("\"saved_mismatches\":0"), "{whole}");
    let dropping = melee_on("melee-drops.scenario", "seconds = 10\nworld.drops = 1\n");
    let out = run(&dropping, &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        said(&out).contains("expected game.saved_mismatches == 0"),
        "{}",
        said(&out)
    );
}

#[test]
fn an_unknown_knob_is_refused_at_its_line() {
    let file = melee_on("melee-knob.scenario", "knobs.speed = 3\n");
    let out = run(&file, &[]);
    assert_eq!(out.status.code(), Some(2));
    let fault = format!("{}:3: `speed` is not a knob of this game", file.display());
    assert_eq!(said(&out).trim(), fault);
}
