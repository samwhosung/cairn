mod authors;
mod crash;
mod damage;
mod reach;
mod readback;
mod relief;
mod rules;
mod steps;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::choice::hash_bytes;
use crate::document::Document;
use crate::files::ZoneFile;
use crate::install::{Install, ModelBox, Rules};
use crate::relief::{Ground, Source};
use crate::zone::{Borrow, Heights, Zone};

pub struct FakeInstall;

impl Install for FakeInstall {
    fn model_box(&mut self, model: &str) -> Result<ModelBox, String> {
        if model.to_ascii_lowercase().ends_with(".wmo") {
            return Ok(ModelBox {
                min: [-12.0, -8.0, 0.0],
                max: [12.0, 8.0, 10.0],
            });
        }
        let side = 1.0 + (crate::choice::hash_ignoring_case(model) % 8) as f32;
        Ok(ModelBox {
            min: [-side / 2.0, -side / 3.0, 0.0],
            max: [side / 2.0, side / 3.0, side * 2.0],
        })
    }

    fn has_texture(&mut self, _: &str) -> Result<bool, String> {
        Ok(true)
    }

    fn zone_named(&mut self, name: &str) -> Result<Borrow, String> {
        Ok(Borrow {
            area: 12,
            name: name.to_owned(),
        })
    }

    fn rules(&mut self, model: &str) -> Result<Option<Rules>, String> {
        let m = model.to_ascii_lowercase();
        Ok(if m.contains("ruledtree") {
            Some(Rules {
                slope: [0.0, 30.0],
                apart: 6.0,
                scale: [0.8, 1.4],
                lean: false,
                placed: 400,
            })
        } else if m.contains("ruledrock") {
            Some(Rules {
                slope: [10.0, 70.0],
                apart: 3.5,
                scale: [0.5, 2.0],
                lean: true,
                placed: 90,
            })
        } else {
            None
        })
    }

    fn relief(&mut self, name: &str) -> Result<Arc<Source>, String> {
        let (cols, rows) = (100, 100);
        let h = |x: f64, y: f64| {
            8.0 * libm::sin(x * 0.37) * libm::cos(y * 0.23) + if x > 50.0 { 12.0 } else { 0.0 }
        };
        let ground = Ground {
            heights: Heights {
                cols,
                rows,
                outer: (0..(cols + 1) * (rows + 1))
                    .map(|k| h((k % (cols + 1)) as f64, (k / (cols + 1)) as f64) as f32)
                    .collect(),
                inner: (0..cols * rows)
                    .map(|k| h((k % cols) as f64 + 0.5, (k / cols) as f64 + 0.5) as f32)
                    .collect(),
            },
            known: vec![true; cols * rows],
        };
        Source::of(name, &ground).map(Arc::new)
    }
}

pub fn scratch(label: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("cairn-document-{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A zone that uses every verb, every flag of a move, four textures in some chunks, a texture
/// made room for, both kinds of thing and both kinds of height.
pub const SCRIPT: &[&str] = &[
    "new --tiles 2x2 --height 40 --texture Tileset\\Elwynn\\ElwynnGrassBase.blp --effect 505 --borrow Elwynn Forest --name Checks",
    "texture Tileset\\Elwynn\\ElwynnDirtBase2.blp --effect 1106",
    "raise 25 --at 300,300 --radius 150 --falloff 1",
    "lower 6 --at 700,650 --radius 110 --falloff 0.8",
    "flatten 52 --at 420,260 --radius 22 --falloff 0.4",
    "roughen 2.5 --size 14 --at 600,300 --radius 160 --seed 3",
    "smooth --at 300,300 --radius 60 --passes 3",
    "flatten --line 40,520 280,330 520,300 900,420 --width 10 --falloff 0.5",
    "paint Tileset\\Elwynn\\ElwynnDirtBase2.blp --line 40,520 280,330 520,300 900,420 --width 7 --falloff 0.3",
    "paint Tileset\\Elwynn\\ElwynnFlowerBase.blp --at 520,300 --radius 60 --strength 0.7",
    "paint Tileset\\Elwynn\\ElwynnRockBaseTest2.blp --at 500,290 --radius 45 --strength 0.8",
    "paint Tileset\\Elwynn\\ElwynnCobbleStoneBase.blp --at 500,310 --radius 30",
    "paint Tileset\\Elwynn\\ElwynnCobbleStoneBase.blp --at 500,310 --radius 30 --make-room",
    "place World\\wmo\\Azeroth\\Buildings\\human_farm\\farm.wmo 420,260 --facing 200 --set 1",
    "place World\\wmo\\Azeroth\\Buildings\\Human_Barn_Silo\\barn.wmo 460,215 --facing 110",
    "place World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTreeCanopy01.m2 300,300 --facing 30",
    "place World\\Azeroth\\Westfall\\PassiveDoodads\\Barrel\\WestFallBarrel01.m2 433,247 --scale 1.2",
    "place World\\Azeroth\\Westfall\\PassiveDoodads\\LampPost\\WestfallLampPost02.m2 395,280 --facing 20 --dz -0.2",
    "place World\\Azeroth\\Elwynn\\PassiveDoodads\\ElwynnFences\\ElwynnWoodFence01.m2 520,250 --facing 90 --z 49.5",
    "water 38 --at 700,650 --radius 100",
    "water 44 --rect 100,850 200,950",
    "set --name Checks Two --start 60,520,60 --borrow Elwynn Forest",
    "move 3 310,305 --turn 45 --scale 1.5 --dz 1",
    "move 2 --set 2 --facing 10",
    "move 5 --z 50",
    "move 5 380,290",
    "remove 4 6",
    "water --remove 8",
    "texture Tileset\\Elwynn\\ElwynnFlowerBase.blp --effect 7",
    "scatter --models World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTreeMid01.m2 World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTree01\\ElwynnPine01.m2 --poly 600,400 900,380 950,700 620,720 --count 80 --apart 9 --scale 0.8..1.3",
    "scatter --models World\\Azeroth\\Elwynn\\PassiveDoodads\\Bush\\ElwynnBush09.m2 --at 520,520 --radius 60 --count 20 --apart 4 --facing 0",
    "raise 3 --line 100,100 900,900 --width 40",
    "paint Tileset\\Elwynn\\ElwynnRockBaseTest2.blp --at 300,300 --radius 160 --falloff 0 --slope 6..90 --soft 3",
    "paint Tileset\\Elwynn\\ElwynnDirtBase2.blp --at 700,650 --radius 130 --water -4..6 --soft 2 --make-room",
    "scatter --models World\\Azeroth\\RuledTree01.m2 World\\Azeroth\\RuledRock02.m2 --at 300,300 --radius 170 --count 70 --water 2.. --off Tileset\\Elwynn\\ElwynnDirtBase2.blp --seed 5",
    "scatter --models World\\Azeroth\\RuledTree01.m2 --at 330,300 --radius 60 --count 12 --seed 5",
    "relief Redridge Mountains --at 720,300 --radius 140 --strength 0.7 --seed 3",
    "place World\\Azeroth\\RuledRock09.m2 310,250 --stands leaning",
    "move 3 --stands leaning",
];

pub fn journal(author: &str, commands: &[&str]) -> Vec<String> {
    commands.iter().map(|c| format!("t {author} {c}")).collect()
}

pub fn make(dir: &Path, lines: &[String]) -> Document {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    Document::replay(&lines, dir, &mut FakeInstall).unwrap_or_else(|e| panic!("{e}"))
}

/// Everything but the authors' counts and the textures added after the first `palette`, which
/// undo never takes out.
pub fn undoable_state(z: &Zone, palette: usize) -> u64 {
    let mut b = Vec::new();
    for p in [
        ZoneFile::Heights,
        ZoneFile::Paint,
        ZoneFile::Things,
        ZoneFile::Water,
    ] {
        b.extend(p.bytes(z));
    }
    let s = &z.settings;
    b.extend(format!("{}|{}|{:?}|{:?}", s.name, s.map, s.start, s.borrow).bytes());
    for t in &z.palette[..palette] {
        b.extend(format!("|{}|{}", t.path, t.effect).bytes());
    }
    hash_bytes(&b)
}

pub fn files(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for p in ZoneFile::ALL {
        let bytes = std::fs::read(dir.join(p.file())).unwrap_or_default();
        out.push((p.file().to_owned(), bytes));
    }
    out
}
