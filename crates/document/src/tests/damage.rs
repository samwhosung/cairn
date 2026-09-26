use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

use super::{FakeInstall, journal, make, scratch};
use crate::document::Document;

const SMALL: &[&str] = &[
    "new --tiles 1x1 --height 40 --texture Tileset\\A.blp --effect 5 --borrow Elwynn Forest --name Damage",
    "raise 8 --at 200,200 --radius 60",
    "paint Tileset\\B.blp --at 200,200 --radius 40",
    "place World\\wmo\\Farm.wmo 300,300 --facing 20",
    "place World\\Tree.m2 250,200 --scale 1.5",
    "water 45 --rect 100,100 150,150",
    "set --start 10,10,90",
    "move 2 260,210",
];

fn exercise(dir: &Path) {
    let Ok(mut doc) = Document::open(dir, &mut FakeInstall) else {
        return;
    };
    let _ = doc.undo("sam");
    let _ = doc.redo("sam");
    let _ = doc.undo("sam");
    let _ = doc.build(&mut FakeInstall);
    let _ = crate::query::things(doc.zone(), &mut FakeInstall, None, None);
    let _ = crate::query::here(doc.zone(), &mut FakeInstall, [200.0, 200.0], 40.0);
}

#[test]
fn damaged_files_give_errors_never_panics() {
    let dir = scratch("damage");
    let mut doc = make(&dir, &journal("sam", SMALL));
    doc.save().unwrap_or_else(|e| panic!("{e}"));
    doc.undo("sam").unwrap_or_else(|e| panic!("{e}"));
    doc.redo("sam").unwrap_or_else(|e| panic!("{e}"));
    doc.replay_line("t sam raise 2 --at 100,100 --radius 20", &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    drop(doc);
    let names: Vec<String> = std::fs::read_dir(&dir)
        .map(|d| {
            d.filter_map(|e| {
                let e = e.ok()?;
                e.file_type()
                    .ok()?
                    .is_file()
                    .then_some(e.file_name().into_string().ok()?)
            })
            .filter(|n| n != ".lock")
            .collect()
        })
        .unwrap_or_default();
    let pristine: Vec<(String, Vec<u8>)> = names
        .iter()
        .map(|n| (n.clone(), std::fs::read(dir.join(n)).unwrap_or_default()))
        .collect();
    let mut tried = 0;
    for (name, bytes) in &pristine {
        let len = bytes.len();
        let mut variants: Vec<Vec<u8>> = [0, 1, len / 3, len / 2, len.saturating_sub(1)]
            .iter()
            .map(|&cut| bytes[..cut.min(len)].to_vec())
            .collect();
        for k in 0..24 {
            let at = (k * 7919 + 13) % len.max(1);
            let mut b = bytes.clone();
            if let Some(v) = b.get_mut(at) {
                *v ^= 1 << (k % 8);
            }
            variants.push(b);
        }
        for v in variants {
            std::fs::write(dir.join(name), &v).unwrap_or_else(|e| panic!("{e}"));
            let ran = catch_unwind(AssertUnwindSafe(|| exercise(&dir)));
            assert!(ran.is_ok(), "{name} damaged made the zone panic");
            tried += 1;
            for (n, b) in &pristine {
                if n == name || n == crate::journal::FILE || n == crate::history::FILE {
                    std::fs::write(dir.join(n), b).unwrap_or_else(|e| panic!("{e}"));
                }
            }
        }
    }
    assert!(tried > 200, "{tried} damaged copies");
}
