use std::collections::BTreeSet;

use super::{FakeInstall, SCRIPT, journal, make, scratch};
use crate::document::Document;
use crate::zone::Id;

fn line(doc: &mut Document, author: &str, command: &str, made: &mut Vec<Id>) -> Result<(), String> {
    let before: BTreeSet<Id> = doc.zone().things.keys().cloned().collect();
    doc.replay_line(&format!("t {author} {command}"), &mut FakeInstall)?;
    made.extend(
        doc.zone()
            .things
            .keys()
            .filter(|id| !before.contains(*id))
            .cloned(),
    );
    Ok(())
}

fn check(control: bool) -> Result<(), String> {
    if control {
        crate::edit::control::share_ids();
    }
    let mut doc = make(&scratch("authors"), &journal("sam", &SCRIPT[..3]));
    let mut made = Vec::new();
    let id = |author: &str, n: u32| Id {
        author: author.into(),
        n,
    };
    line(
        &mut doc,
        "ai",
        "paint Tileset\\B.blp --at 300,300 --radius 30",
        &mut made,
    )?;
    line(&mut doc, "sam", "place A.m2 100,100", &mut made)?;
    line(&mut doc, "ai", "place B.m2 200,200", &mut made)?;
    let (a, b) = (made[0].clone(), made[1].clone());
    line(&mut doc, "ai", &format!("move {b} 210,200"), &mut made)?;
    line(
        &mut doc,
        "sam",
        "scatter --models C.m2 --at 400,400 --radius 40 --count 12 --apart 6",
        &mut made,
    )?;
    let scattered = doc.zone().things.len() - 2;
    doc.undo("ai")?;
    if doc.zone().things[&b].x != 20_000 || doc.zone().things.len() != scattered + 2 {
        return Err("ai's undo took back more than ai's move".into());
    }
    doc.undo("sam")?;
    if doc.zone().things.len() != 2 || !doc.zone().things.contains_key(&b) {
        return Err("sam's undo took back more than sam's scatter".into());
    }
    line(&mut doc, "ai", &format!("move {a} 110,100"), &mut made)?;
    match doc.undo("sam") {
        Err(e) if e.contains(&format!("ai changed thing {a}")) => {}
        other => return Err(format!("an undo across ai's move of {a} went {other:?}")),
    }
    doc.undo("ai")?;
    doc.undo("sam")?;
    if doc.zone().things.contains_key(&a) {
        return Err(format!("sam's undo left {a}"));
    }
    line(&mut doc, "ai", "place D.m2 300,300", &mut made)?;
    doc.redo("sam")?;
    doc.redo("sam")?;
    let once: BTreeSet<&Id> = made.iter().collect();
    if once.len() != made.len() {
        return Err(format!("an id was handed out twice: {made:?}"));
    }
    if !control && (made[0] != id("sam", 1) || made[1] != id("ai", 1)) {
        return Err(format!("ids are not their author's own count: {made:?}"));
    }
    Ok(())
}

#[test]
fn each_author_undoes_only_their_own_and_no_id_comes_round_again() {
    check(false).unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn the_control_largest_plus_one_hands_an_id_out_twice() {
    let e = check(true).expect_err("the check must fail");
    println!("{e}");
    assert!(
        e.contains("twice") || e.contains("refused") || e.contains("can't"),
        "{e}"
    );
}
