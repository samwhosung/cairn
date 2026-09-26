use std::sync::Arc;

use super::{FakeInstall, journal, make, scratch};
use crate::document::Document;
use crate::install::{Install, ModelBox, Rules};
use crate::journal::{Entry, parse};
use crate::relief::Source;
use crate::zone::{Borrow, Id, Zone};

const TREE: &str = "World\\Azeroth\\RuledTree01.m2";
const ROCK: &str = "World\\Azeroth\\RuledRock02.m2";

const HILL: &[&str] = &[
    "new --tiles 1x1 --height 40 --texture Tileset\\Grass.blp --borrow Elwynn Forest --name Hill",
    "raise 60 --at 260,200 --radius 110 --falloff 0.35",
    "lower 12 --at 260,420 --radius 60 --falloff 0.6",
    "water 36 --at 260,420 --radius 70",
    "paint Tileset\\Road.blp --line 0,300 533,300 --width 12 --falloff 0",
];

struct Unruled;

impl Install for Unruled {
    fn model_box(&mut self, model: &str) -> Result<ModelBox, String> {
        FakeInstall.model_box(model)
    }

    fn has_texture(&mut self, path: &str) -> Result<bool, String> {
        FakeInstall.has_texture(path)
    }

    fn zone_named(&mut self, name: &str) -> Result<Borrow, String> {
        FakeInstall.zone_named(name)
    }

    fn rules(&mut self, _: &str) -> Result<Option<Rules>, String> {
        Ok(None)
    }

    fn relief(&mut self, name: &str) -> Result<Arc<Source>, String> {
        FakeInstall.relief(name)
    }
}

fn hill(more: &[&str]) -> Document {
    let mut lines = HILL.to_vec();
    lines.extend_from_slice(more);
    make(&scratch("rules"), &journal("sam", &lines))
}

fn things_of<'a>(z: &'a Zone, model: &str) -> Vec<(&'a Id, &'a crate::zone::Thing)> {
    z.things.iter().filter(|(_, t)| t.model == model).collect()
}

#[test]
fn a_scatter_keeps_each_models_rules_and_the_journal_keeps_them() {
    let doc = hill(&[&format!(
        "scatter --models {TREE} {ROCK} --rect 60,40 460,500 --count 400 --water 1.. --off Tileset\\Road.blp"
    )]);
    let z = doc.zone();
    let (trees, rocks) = (things_of(z, TREE), things_of(z, ROCK));
    assert!(
        trees.len() > 30 && rocks.len() > 30,
        "{} {}",
        trees.len(),
        rocks.len()
    );
    let slope = |t: &crate::zone::Thing| z.heights.slope(t.at());
    assert!(trees.iter().all(|(_, t)| slope(t) <= 31.5 && !t.lean));
    assert!(rocks.iter().all(|(_, t)| slope(t) >= 8.5 && t.lean));
    let steep = z.things.values().filter(|t| slope(t) > 30.5).count();
    assert!(steep > 0, "rocks stand where trees may not");
    for (_, t) in trees.iter().chain(&rocks) {
        let p = t.at();
        let wet = z.water.values().any(|w| {
            let (i, j) = (
                (p[0] / crate::frame::CELL) as usize,
                (p[1] / crate::frame::CELL) as usize,
            );
            crate::build::wets(w, &z.heights, i, j)
        });
        assert!(!wet, "{p:?} is in the water");
        assert!((p[1] - 300.0).abs() > 5.4, "{p:?} is on the road");
    }
    let last = doc.lines().last().cloned().unwrap_or_default();
    assert!(
        last.contains("--slope 0..30 10..70") && last.contains("--stands upright leaning"),
        "{last}"
    );
    let Entry::Command { command, .. } = parse(&last).map_or(Entry::Undo, |l| l.entry) else {
        panic!("{last}");
    };
    let lines: Vec<&str> = doc.lines().iter().map(String::as_str).collect();
    let again = Document::replay(&lines, &scratch("rules-again"), &mut Unruled)
        .unwrap_or_else(|e| panic!("the journal alone makes it again: {e}"));
    assert_eq!(again.zone(), z, "{command}");
}

#[test]
fn without_rules_a_scatter_lands_as_it_always_did() {
    let words = format!("scatter --models {TREE} --rect 60,40 460,500 --count 300 --apart 6");
    let ruled = hill(&[&words]);
    let lines = journal("sam", &[HILL, &[words.as_str()]].concat());
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let unruled = Document::replay(&lines, &scratch("unruled"), &mut Unruled)
        .unwrap_or_else(|e| panic!("{e}"));
    let steep = |d: &Document| {
        let z = d.zone();
        z.things
            .values()
            .filter(|t| z.heights.slope(t.at()) > 31.5)
            .count()
    };
    assert_eq!(steep(&ruled), 0, "the rules keep trees off the flanks");
    assert!(steep(&unruled) > 10, "{}", steep(&unruled));
    assert!(
        unruled
            .zone()
            .things
            .values()
            .all(|t| !t.lean && t.scale == 1024)
    );
}

#[test]
fn a_scatter_again_replaces_what_it_placed_and_nothing_else() {
    let fewer = format!("scatter --models {TREE} --rect 60,440 460,520 --count 5");
    let words = fewer.replace("--count 5", "--count 40");
    let mut doc = hill(&[
        words.as_str(),
        &format!("place {TREE} 100,480"),
        "move sam.9 --dz -1",
    ]);
    let once = things_of(doc.zone(), TREE).len();
    doc.replay_line(&format!("t sam {words}"), &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        things_of(doc.zone(), TREE).len(),
        once + 1,
        "the same scatter lands once more only where one was moved by hand"
    );
    let hand: Vec<&Id> = doc
        .zone()
        .things
        .iter()
        .filter(|(_, t)| t.scattered.is_none())
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        hand.len(),
        2,
        "the one placed and the one moved by hand stay: {hand:?}"
    );
    doc.replay_line(&format!("t sam {words} --seed 2"), &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(
        things_of(doc.zone(), TREE).len() > once,
        "another seed adds"
    );
    let before = doc.zone().clone();
    doc.replay_line(&format!("t sam {fewer}"), &mut FakeInstall)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        things_of(doc.zone(), TREE).len(),
        things_of(&before, TREE).len() - (once - 1) + 5,
        "fewer, the second time"
    );
    doc.undo("sam").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        doc.zone().things,
        before.things,
        "and undone, the first again"
    );
}

#[test]
fn a_mask_keeps_a_stroke_to_its_ground() {
    let rock = "Tileset\\Rock.blp";
    let doc = hill(&[&format!(
        "paint {rock} --rect 0,0 533,533 --slope 30..90 --soft 4"
    )]);
    let z = doc.zone();
    let place = z
        .palette_place(rock)
        .unwrap_or_else(|| panic!("in the palette"));
    let (mut on_steep, mut on_flat, mut eased) = (0, 0, 0);
    let east = z.chunks_east();
    for (c, p) in z.paint.iter().enumerate() {
        let Some(l) = p.layers.iter().find(|l| l.palette_place == place) else {
            continue;
        };
        for (t, &w) in l.w.iter().enumerate() {
            let at = [
                (c % east) as f64 * crate::frame::CHUNK
                    + ((t % 64) as f64 + 0.5) * crate::frame::TEXEL,
                (c / east) as f64 * crate::frame::CHUNK
                    + ((t / 64) as f64 + 0.5) * crate::frame::TEXEL,
            ];
            let s = z.heights.slope(at);
            match (s, w) {
                (s, 255) if s >= 32.0 => on_steep += 1,
                (s, w) if s <= 28.0 && w > 0 => on_flat += 1,
                (_, w) if w > 0 && w < 255 => eased += 1,
                _ => {}
            }
        }
    }
    assert!(on_steep > 1000 && eased > 0, "{on_steep} {eased}");
    assert_eq!(on_flat, 0, "no rock on ground under 28°");
}
