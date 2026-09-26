use std::time::Instant;

use super::{FakeInstall, SCRIPT, files, journal, make, scratch, undoable_state};
use crate::document::Document;
use crate::journal::{Kind, head};

struct Timed {
    undo_ms: Vec<f64>,
    redo_ms: Vec<f64>,
}

fn every_step_of(lines: &[String], control: bool) -> Result<Timed, String> {
    if control {
        crate::image::control::drop_facing();
    }
    let dir = scratch("steps");
    let mut doc = make(&dir, &lines[..1]);
    let palette = |d: &Document| d.zone().palette.len();
    let mut states = vec![(undoable_state(doc.zone(), palette(&doc)), palette(&doc))];
    let mut authors = vec![String::new()];
    for l in &lines[1..] {
        let h = head(l)?;
        if h.kind != Kind::Step {
            return Err(format!("a journal of commands alone, not `{l}`"));
        }
        doc.replay_line(l, &mut FakeInstall)?;
        states.push((undoable_state(doc.zone(), palette(&doc)), palette(&doc)));
        authors.push(h.author);
    }
    doc.save()?;
    let before = files(&dir);
    let mut timed = Timed {
        undo_ms: Vec::new(),
        redo_ms: Vec::new(),
    };
    for k in (1..states.len()).rev() {
        let t = Instant::now();
        doc.undo(&authors[k])?;
        timed.undo_ms.push(t.elapsed().as_secs_f64() * 1e3);
        let (want, palette) = states[k - 1];
        if undoable_state(doc.zone(), palette) != want {
            return Err(format!(
                "undoing line {} left the zone unlike it was before: {}",
                k + 1,
                lines[k]
            ));
        }
    }
    if authors.iter().skip(1).any(|a| doc.undo(a).is_ok()) {
        return Err("an undo past the first step".into());
    }
    for (k, &(want, palette)) in states.iter().enumerate().skip(1) {
        let t = Instant::now();
        doc.redo(&authors[k])?;
        timed.redo_ms.push(t.elapsed().as_secs_f64() * 1e3);
        if undoable_state(doc.zone(), palette) != want {
            return Err(format!(
                "redoing line {} left the zone unlike it was after: {}",
                k + 1,
                lines[k]
            ));
        }
    }
    doc.save()?;
    if files(&dir) != before {
        return Err("the files differ after undoing and redoing every step".into());
    }
    drop(doc);
    let doc = Document::open(&dir, &mut FakeInstall)?;
    let (want, palette) = states[states.len() - 1];
    if undoable_state(doc.zone(), palette) != want {
        return Err("the zone reopened differs".into());
    }
    Ok(timed)
}

#[test]
fn every_step_undoes_and_redoes_exactly() {
    every_step_of(&journal("sam", SCRIPT), false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_control_an_inverse_that_drops_a_field_is_caught() {
    let e = every_step_of(&journal("sam", SCRIPT), true)
        .err()
        .unwrap_or_default();
    assert!(e.contains("move"), "the check must fail at a move: {e:?}");
}

#[test]
#[ignore = "a measurement over the journal CAIRN_JOURNAL names, for a release build"]
fn a_journal_undoes_and_redoes_every_step() {
    let Some(path) = std::env::var_os("CAIRN_JOURNAL") else {
        eprintln!("skipped: CAIRN_JOURNAL is not set");
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{e}"));
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let t = every_step_of(&lines, false).unwrap_or_else(|e| panic!("{e}"));
    for (what, ms) in [("undo", &t.undo_ms), ("redo", &t.redo_ms)] {
        let mut sorted = ms.clone();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        println!(
            "{what}: {} steps, {:.1} ms in all; median {:.3} ms, 99th {:.3} ms, slowest {:.3} ms",
            ms.len(),
            ms.iter().sum::<f64>(),
            at(0.5),
            at(0.99),
            at(1.0)
        );
    }
    let e = every_step_of(&lines, true).err().unwrap_or_default();
    println!("the control, an inverse that drops a thing's facing: {e}");
    assert!(!e.is_empty(), "the control must fail");
}

#[test]
fn a_drag_is_one_step_and_a_batch_all_or_nothing() {
    let dir = scratch("drag");
    let mut doc = make(&dir, &journal("sam", &SCRIPT[..16]));
    let tree = crate::zone::Id {
        author: "sam".into(),
        n: 3,
    };
    let was = doc.zone().things[&tree].clone();
    doc.replay_line("t sam move 3 301,300", &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    for x in 302..310 {
        doc.replay_line(&format!("t sam + move 3 {x},300"), &mut FakeInstall)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    assert_eq!(doc.zone().things[&tree].x, 30_900);
    let said = doc.undo("sam").unwrap_or_else(|e| panic!("{e}"));
    assert!(said.reply.contains("and 8 more"), "{}", said.reply);
    assert_eq!(said.footprint.things.iter().collect::<Vec<_>>(), [&tree]);
    assert_eq!(doc.zone().things[&tree], was);
    let lines = doc.lines().len();
    let things = doc.zone().things.len();
    let place = |x: u32| {
        crate::command::Command::parse(
            &crate::grammar::split(&format!("place A.m2 {x},10")).unwrap_or_default(),
            "sam",
        )
        .unwrap_or_else(|e| panic!("{e}"))
    };
    let bad = crate::command::Command::parse(
        &crate::grammar::split("move 999 1,1").unwrap_or_default(),
        "sam",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let refused = doc.batch("sam", &[place(10), place(20), bad], &mut FakeInstall);
    assert!(refused.is_err_and(|e| e.contains("nothing of the batch")));
    assert_eq!(
        (doc.lines().len(), doc.zone().things.len()),
        (lines, things)
    );
    doc.batch("sam", &[place(10), place(20)], &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(doc.zone().things.len(), things + 2);
    doc.undo("sam").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(doc.zone().things.len(), things);
}
