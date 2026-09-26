use super::{FakeInstall, SCRIPT, journal, make, scratch, undoable_state};
use crate::document::Document;
use crate::files::ZoneFile;

fn replayed(lines: &[String]) -> u64 {
    let doc = make(&scratch("replayed"), lines);
    undoable_state(doc.zone(), doc.zone().palette.len())
}

fn reopened(dir: &std::path::Path) -> Document {
    Document::open(dir, &mut FakeInstall).unwrap_or_else(|e| panic!("{e}"))
}

fn now(doc: &Document) -> u64 {
    undoable_state(doc.zone(), doc.zone().palette.len())
}

#[test]
fn the_files_roll_forward_to_the_journal() {
    let dir = scratch("roll");
    let lines = journal("sam", SCRIPT);
    let mut doc = make(&dir, &lines[..12]);
    doc.save().unwrap_or_else(|e| panic!("{e}"));
    for l in &lines[12..] {
        doc.replay_line(l, &mut FakeInstall)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    for l in ["t sam undo", "t sam undo", "t sam redo"] {
        doc.replay_line(l, &mut FakeInstall)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let written: Vec<String> = doc.lines().to_vec();
    drop(doc);
    assert_eq!(now(&reopened(&dir)), replayed(&written));
}

#[test]
fn a_torn_last_line_and_its_record_are_dropped() {
    let dir = scratch("torn");
    let lines = journal("sam", SCRIPT);
    let mut doc = make(&dir, &lines[..20]);
    doc.save().unwrap_or_else(|e| panic!("{e}"));
    let before = now(&doc);
    doc.replay_line(&lines[20], &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    drop(doc);
    let path = dir.join(crate::journal::FILE);
    let text = std::fs::read(&path).unwrap_or_default();
    let cut = text.len() - 7;
    std::fs::write(&path, &text[..cut]).unwrap_or_else(|e| panic!("{e}"));
    let doc = reopened(&dir);
    assert_eq!(now(&doc), before);
    assert_eq!(doc.lines().len(), 20);
    assert_eq!(now(&doc), replayed(&lines[..20]));
}

#[test]
fn a_save_cut_short_is_finished_or_forgotten() {
    let dir = scratch("save");
    let lines = journal("sam", SCRIPT);
    let mut doc = make(&dir, &lines[..20]);
    doc.save().unwrap_or_else(|e| panic!("{e}"));
    for l in &lines[20..25] {
        doc.replay_line(l, &mut FakeInstall)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let after = now(&doc);
    let mut saving = String::from("25\n");
    for p in ZoneFile::ALL {
        std::fs::write(dir.join(format!(".{}.new", p.file())), p.bytes(doc.zone()))
            .unwrap_or_else(|e| panic!("{e}"));
        saving.push_str(p.file());
        saving.push('\n');
    }
    std::fs::write(dir.join(".saving"), saving).unwrap_or_else(|e| panic!("{e}"));
    std::fs::rename(dir.join(".things.txt.new"), dir.join("things.txt"))
        .unwrap_or_else(|e| panic!("{e}"));
    drop(doc);
    let doc = reopened(&dir);
    assert_eq!(now(&doc), after);
    drop(doc);
    std::fs::write(dir.join(".heights.bin.new"), b"half a save").unwrap_or_else(|e| panic!("{e}"));
    let doc = reopened(&dir);
    assert_eq!(now(&doc), after);
    assert!(!dir.join(".heights.bin.new").exists());
}

#[test]
fn a_zone_made_but_never_saved_is_made_again() {
    let dir = scratch("unsaved");
    let lines = journal("sam", SCRIPT);
    let doc = make(&dir, &lines[..9]);
    drop(doc);
    for f in ZoneFile::ALL
        .map(ZoneFile::file)
        .into_iter()
        .chain([crate::files::SAVED])
    {
        std::fs::remove_file(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
    assert_eq!(now(&reopened(&dir)), replayed(&lines[..9]));
}

#[test]
fn the_control_a_journal_held_back_loses_commands() {
    let dir = scratch("held");
    let lines = journal("sam", SCRIPT);
    let mut doc = make(&dir, &lines[..10]);
    doc.save().unwrap_or_else(|e| panic!("{e}"));
    doc.hold_journal();
    for l in &lines[10..15] {
        doc.replay_line(l, &mut FakeInstall)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    let answered = now(&doc);
    drop(doc);
    let doc = reopened(&dir);
    assert_ne!(now(&doc), answered, "held lines should be lost");
    assert_eq!(doc.lines().len(), 10);
}
