//! The scenario files, run as an agent runs them: `bots scenario FILE`, one verdict line out.

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

/// The verdict without what only this run measured here: its wall time, speed, threads, cost of
/// a tick and the machine's load.
fn measured(out: &Output) -> String {
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

/// A scenario written for one test, beside the others' in the test's scratch.
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
        let one = measured(&run(&file, &["--threads", "1"]));
        let many = measured(&run(&file, &["--threads", "14"]));
        assert_eq!(one, many, "{name}");
        let racy = measured(&run(&file, &["--threads", "14", "--racy"]));
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
fn an_unknown_key_is_refused_at_its_line_and_a_known_one_is_taken() {
    let unknown = scratch(
        "unknown.scenario",
        &over("flat.scenario", "seconds = 5\nrules.speed = 7\n"),
    );
    let out = run(&unknown, &[]);
    assert_eq!(out.status.code(), Some(2));
    let fault = format!(
        "{}:3: `rules.speed` is not a key a scenario sets",
        unknown.display()
    );
    assert_eq!(said(&out).trim(), fault);
    let known = scratch(
        "known.scenario",
        &over("flat.scenario", "seconds = 5\nrules.run = 7\n"),
    );
    let out = run(&known, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
}
