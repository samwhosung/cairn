use std::collections::BTreeMap;

use super::{FakeInstall, journal, make, scratch};
use crate::choice::stream;
use crate::frame::{CHUNK, TEXEL};
use crate::grammar::{Args, area, point, split};
use crate::install::Install;
use crate::shape::Shape;
use crate::zone::{TEXELS_ACROSS, Zone};

/// A zone with no moves or removes, since an edit put in early gives its author's later things
/// other counts.
const BASE: &[&str] = &[
    "new --tiles 2x2 --height 40 --texture Tileset\\A.blp --effect 505 --borrow Elwynn Forest --name Reach",
    "texture Tileset\\B.blp --effect 1106",
    "raise 25 --at 300,300 --radius 150 --falloff 1",
    "lower 6 --at 700,650 --radius 110 --falloff 0.8",
    "flatten 52 --at 420,260 --radius 22 --falloff 0.4",
    "roughen 2.5 --size 14 --at 600,300 --radius 160 --seed 3",
    "smooth --at 300,300 --radius 60 --passes 3",
    "flatten --line 40,520 280,330 520,300 900,420 --width 10 --falloff 0.5",
    "paint Tileset\\B.blp --line 40,520 280,330 520,300 900,420 --width 7 --falloff 0.3",
    "paint Tileset\\C.blp --at 520,300 --radius 60 --strength 0.7",
    "paint Tileset\\D.blp --at 500,290 --radius 45 --strength 0.8",
    "place World\\wmo\\Farm.wmo 420,260 --facing 200 --set 1",
    "place World\\Tree.m2 300,300 --facing 30",
    "place World\\Post.m2 395,280 --facing 20 --dz -0.2",
    "scatter --models World\\Mid.m2 World\\Pine.m2 --poly 600,400 900,380 950,700 620,720 --count 80 --apart 9 --scale 0.8..1.3",
    "scatter --models World\\Bush.m2 --at 520,520 --radius 60 --count 20 --apart 4 --facing 0",
    "water 38 --at 700,650 --radius 100",
    "water 44 --rect 100,850 200,950",
];

const PUT_IN_AFTER_LINE: usize = 7;

const EDITS: &[&str] = &[
    "raise 8 --at 800,200 --radius 50",
    "lower 5 --line 150,600 250,700 --width 30",
    "flatten 45 --at 150,650 --radius 30",
    "smooth --at 640,330 --radius 40",
    "roughen 3 --size 10 --at 850,850 --radius 60",
    "paint Tileset\\C.blp --at 200,800 --radius 40",
    "place World\\Canopy.m2 150,150",
    "scatter --models World\\Pine.m2 --rect 100,100 250,250 --count 15 --apart 8",
    "water 45 --rect 850,100 950,200",
    "paint Tileset\\C.blp --at 380,190 --radius 45 --slope 3..90 --soft 2",
    "scatter --models World\\RuledTree.m2 World\\RuledRock.m2 --at 650,180 --radius 60 --count 30 --water 1..",
    "relief Elwynn Forest --at 800,150 --radius 70 --strength 0.8",
];

fn zone(commands: &[&str], control: bool) -> Zone {
    if control {
        stream::start();
    }
    make(&scratch("reach"), &journal("sam", commands))
        .zone()
        .clone()
}

fn outside(s: &Shape, p: [f64; 2]) -> f64 {
    if s.reach(p).is_some() {
        return 0.0;
    }
    let [lo, hi] = s.bounds();
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    let to_box = (dx * dx + dy * dy).sqrt();
    match s {
        Shape::Circle { c, r } => {
            (((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt() - r).max(0.0)
        }
        _ => to_box.max(1e-3),
    }
}

fn reach_of(edit: &str) -> Shape {
    let w = split(edit).unwrap_or_default();
    let a = Args::parse(&w[1..]);
    if w[0] == "place" {
        let b = FakeInstall
            .model_box(&a.pos[0])
            .unwrap_or_else(|e| panic!("{e}"));
        let s = b.size();
        return Shape::Circle {
            c: point(&a.pos[1]).unwrap_or_else(|e| panic!("{e}")),
            r: f64::from(s[0].max(s[1])),
        };
    }
    area(&a).unwrap_or_else(|e| panic!("{e}"))
}

fn changes(a: &Zone, b: &Zone) -> Vec<(&'static str, [f64; 2])> {
    let mut out = Vec::new();
    for k in 0..a.heights.len() {
        if a.heights.get(k).to_bits() != b.heights.get(k).to_bits() {
            out.push(("height", a.heights.point(k)));
        }
    }
    let east = a.chunks_east();
    for (c, (pa, pb)) in a.paint.iter().zip(&b.paint).enumerate() {
        let weights = |z: &Zone, p: &crate::zone::ChunkPaint, t: usize| -> BTreeMap<String, u8> {
            p.layers
                .iter()
                .map(|l| (z.palette[usize::from(l.palette_place)].path.clone(), l.w[t]))
                .filter(|&(_, w)| w > 0)
                .collect()
        };
        for t in 0..TEXELS_ACROSS * TEXELS_ACROSS {
            if weights(a, pa, t) != weights(b, pb, t) {
                let p = [
                    (c % east) as f64 * CHUNK + ((t % TEXELS_ACROSS) as f64 + 0.5) * TEXEL,
                    (c / east) as f64 * CHUNK + ((t / TEXELS_ACROSS) as f64 + 0.5) * TEXEL,
                ];
                out.push(("paint", p));
            }
        }
    }
    let things =
        |z: &Zone| -> Vec<String> { z.things.values().map(crate::text::thing_text).collect() };
    let (ta, tb) = (things(a), things(b));
    for (mine, theirs) in [(&ta, &tb), (&tb, &ta)] {
        for t in mine.iter().filter(|t| !theirs.contains(t)) {
            let t = crate::text::parse_thing(t).unwrap_or_else(|e| panic!("{e}"));
            out.push(("thing", t.at()));
        }
    }
    let waters =
        |z: &Zone| -> Vec<String> { z.water.values().map(crate::text::water_text).collect() };
    let (wa, wb) = (waters(a), waters(b));
    for (mine, theirs) in [(&wa, &wb), (&wb, &wa)] {
        for w in mine.iter().filter(|w| !theirs.contains(w)) {
            let [lo, hi] = crate::text::parse_water(w)
                .unwrap_or_else(|e| panic!("{e}"))
                .shape
                .bounds();
            out.push((
                "water",
                [f64::midpoint(lo[0], hi[0]), f64::midpoint(lo[1], hi[1])],
            ));
        }
    }
    out
}

fn run(control: bool) -> Vec<(usize, f64)> {
    let base = zone(BASE, control);
    EDITS
        .iter()
        .map(|edit| {
            let mut commands = BASE.to_vec();
            commands.insert(PUT_IN_AFTER_LINE, edit);
            let with = zone(&commands, control);
            let reach = reach_of(edit);
            let found = changes(&base, &with);
            let farthest = found
                .iter()
                .map(|(_, p)| outside(&reach, *p))
                .fold(0.0, f64::max);
            (found.len(), farthest)
        })
        .collect()
}

#[test]
fn an_edit_changes_nothing_outside_its_reach() {
    for (edit, (n, farthest)) in EDITS.iter().zip(run(false)) {
        println!("{edit}: {n} changes, the farthest {farthest} yd outside it");
        assert!(n > 0, "{edit} changed nothing");
        assert!(
            farthest == 0.0,
            "{edit} reached {farthest} yd outside itself"
        );
    }
}

#[test]
fn the_control_one_stream_reaches_far() {
    let leaks: Vec<f64> = run(true).into_iter().map(|(_, far)| far).collect();
    for (edit, far) in EDITS.iter().zip(&leaks) {
        println!("one stream, {edit}: the farthest {far:.0} yd outside it");
    }
    assert!(
        leaks.iter().any(|&far| far > 100.0),
        "one stream should move things far: {leaks:?}"
    );
}
