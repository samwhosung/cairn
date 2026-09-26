use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fits::{Spot, Stand, Tables};

use super::catalog::{Catalog, GROUND, Match, words};
use super::lists::{self, Lists};
use super::pictures::decode;
use super::rank::ground_order;
use crate::catalog::what_fits::Found;

mod view;

const BARREL: &str = "WORLD\\GENERIC\\PASSIVEDOODADS\\BARREL\\BARREL01.MDX";
const OAK: &str = "WORLD\\AZEROTH\\ELWYNN\\PASSIVEDOODADS\\TREES\\ELWYNNTREEMID01.MDX";
const STREETLAMP: &str = "WORLD\\GENERIC\\HUMAN\\PASSIVE DOODADS\\STREETLAMPS\\STREETLAMP01.MDX";
const PINE: &str = "WORLD\\AZEROTH\\DUSKWOOD\\PASSIVEDOODADS\\TREES\\DUSKWOODTREE01.MDX";
const GRASS: &str = "Tileset\\Elwynn\\ElwynnGrassBase.blp";
const DIRT: &str = "Tileset\\Elwynn\\ElwynnDirtBase.blp";
const ROAD: &str = "Tileset\\Elwynn\\ElwynnCobblestoneBase.blp";
const TREE: usize = 0;
const PROP: usize = 4;
const ELWYNN: usize = 0;

type ModelRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);
type GroundRow = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const MODELS: [ModelRow; 4] = [
    (
        "prop",
        BARREL,
        "1.2 yd tall, 0.8 x 0.8 across",
        "1,234 (56 inside buildings)",
        "Elwynn Forest 139, Westfall 20 (3 inside buildings)",
    ),
    (
        "tree",
        OAK,
        "9.1 yd tall, 6.0 x 6.2 across",
        "812",
        "Elwynn Forest 812",
    ),
    (
        "prop",
        STREETLAMP,
        "4.2 yd tall, 1.0 x 1.0 across",
        "96",
        "Stormwind City 90, Un'Goro Crater 6",
    ),
    (
        "tree",
        PINE,
        "12.0 yd tall, 5.0 x 5.1 across",
        "1,234",
        "Duskwood 1,234",
    ),
];
const GROUNDS: [GroundRow; 3] = [
    (
        "grass",
        GRASS,
        "2,005",
        "Elwynn Forest 23%, Westfall under 0.1%",
        "ElwynnDirtBase 40%",
    ),
    (
        "dirt",
        DIRT,
        "1,500",
        "Elwynn Forest 5.5%",
        "ElwynnGrassBase 60%",
    ),
    (
        "road",
        ROAD,
        "900",
        "Elwynn Forest 12%",
        "ElwynnGrassBase 10%",
    ),
];

/// A catalog of four models and three ground textures as `cairn catalog` writes one, with plain
/// pictures of its own.
pub(super) fn a_catalog(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cairn-palette-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut tsv = String::from("kind\tpath\tsize\tplaced\tscales\tzones\tpicture\n");
    for (i, (kind, path, size, placed, zones)) in MODELS.iter().enumerate() {
        let picture = format!("models/{}.png", survey::key(path));
        let _ = writeln!(
            tsv,
            "{kind}\t{path}\t{size}\t{placed}\t1.00\t{zones}\t{picture}"
        );
        square(&dir.join(&picture), 480, i);
    }
    write(&dir.join("models.tsv"), &tsv);
    let mut tsv = String::from("kind\tpath\tchunks\tzones\tbeside\tswatch\n");
    for (i, (kind, path, chunks, zones, beside)) in GROUNDS.iter().enumerate() {
        let swatch = format!("ground/{}.png", survey::key(path));
        let _ = writeln!(tsv, "{kind}\t{path}\t{chunks}\t{zones}\t{beside}\t{swatch}");
        square(&dir.join(&swatch), 240, MODELS.len() + i);
    }
    write(&dir.join("ground.tsv"), &tsv);
    fits::write(&dir.join("fits"), tables).expect("the tables");
    dir
}

fn tables() -> Tables {
    let models = MODELS
        .iter()
        .map(|(kind, path, ..)| fits::Model {
            kind: (*kind).to_owned(),
            path: (*path).to_owned(),
        })
        .collect();
    let zones = [(12, "Elwynn Forest"), (10, "Duskwood")]
        .map(|(area, name)| fits::Zone {
            map: 0,
            area,
            key: name.to_ascii_lowercase().replace(' ', "-"),
            name: name.to_owned(),
        })
        .to_vec();
    let stands: Vec<Stand> = (0..40)
        .map(|i| Stand {
            model: i % 4,
            zone: usize::from(i % 4 == 3),
            at: [i as f32 * 3.0, 0.0],
            ground: Some((0, 0)),
        })
        .collect();
    Tables::count(models, vec![GRASS.to_owned()], zones, &stands)
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the folder");
    std::fs::write(path, text).expect("the file");
}

/// A square picture `side` across, framed, of a colour its number picks.
fn square(path: &Path, side: u32, n: usize) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the folder");
    let n = n as u8;
    let picture = image::RgbImage::from_fn(side, side, |x, y| {
        let frame = x < 8 || y < 8 || x >= side - 8 || y >= side - 8;
        if frame {
            image::Rgb([230, 226, 212])
        } else {
            image::Rgb([40 * (n % 6), 90 + 25 * (n % 4), 200 - 30 * (n % 5)])
        }
    });
    picture.save(path).expect("the picture");
}

#[test]
fn a_search_is_its_words_in_lowercase() {
    assert_eq!(words("  Elwynn   TREE "), vec!["elwynn", "tree"]);
}

#[test]
fn a_catalog_reads_as_its_rows_say() {
    let dir = a_catalog("rows");
    let c = Catalog::read(&dir).expect("the catalog");
    assert_eq!((c.items.len(), c.models), (7, 4));
    let barrel = &c.items[0];
    assert_eq!((barrel.kind, barrel.placed), (PROP, 1234));
    assert_eq!(
        barrel.zones,
        vec![
            ("Elwynn Forest".to_owned(), 139.0),
            ("Westfall".to_owned(), 20.0)
        ]
    );
    let grass = &c.items[4];
    assert_eq!((grass.kind, grass.placed), (GROUND, 2005));
    assert_eq!(
        grass.zones,
        vec![
            ("Elwynn Forest".to_owned(), 0.23),
            ("Westfall".to_owned(), 0.0005)
        ]
    );
    assert_eq!(grass.beside, vec![("ElwynnDirtBase".to_owned(), 0.4)]);
    assert_eq!(
        c.find("world/generic/passivedoodads/barrel/barrel01.m2"),
        Some(0)
    );
    assert_eq!(c.find("tileset/elwynn/elwynngrassbase"), Some(4));
    assert_eq!(c.find("World\\Nothing.mdx"), None);
    for item in 0..4 {
        assert_eq!(c.item_of_model(item), Some(item));
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_catalog_without_its_tables_says_how_to_make_them() {
    let dir = a_catalog("untabled");
    std::fs::remove_dir_all(dir.join("fits")).expect("the tables go");
    let Err(e) = Catalog::read(&dir) else {
        panic!("a catalog without its tables reads");
    };
    assert!(e.contains("`cairn catalog` writes them"), "{e}");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_plain_order_is_the_most_placed_first_then_by_path() {
    let dir = a_catalog("plain");
    let c = Catalog::read(&dir).expect("the catalog");
    let names =
        |order: Vec<usize>| -> Vec<&str> { order.iter().map(|&i| c.items[i].name()).collect() };
    assert_eq!(
        names(c.plain(None)),
        [
            "DUSKWOODTREE01",
            "BARREL01",
            "ELWYNNTREEMID01",
            "STREETLAMP01"
        ],
        "a tie goes to the path first by key: world/azeroth before world/generic"
    );
    assert_eq!(
        names(c.plain(Some(TREE))),
        ["DUSKWOODTREE01", "ELWYNNTREEMID01"]
    );
    assert_eq!(
        names(c.plain(Some(GROUND))),
        ["ElwynnGrassBase", "ElwynnDirtBase", "ElwynnCobblestoneBase"]
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_search_finds_every_word_in_a_name_a_path_a_kind_or_a_zone_s_whole_word() {
    let dir = a_catalog("search");
    let c = Catalog::read(&dir).expect("the catalog");
    let found = |search: &str| -> Vec<(&str, Match)> {
        let words = words(search);
        c.items
            .iter()
            .filter_map(|i| Some((i.name(), i.found(&words)?)))
            .collect()
    };
    assert_eq!(found("barrel"), [("BARREL01", Match::Name)]);
    assert_eq!(found("elwynn trees"), [("ELWYNNTREEMID01", Match::Path)]);
    assert_eq!(found("duskwood tree"), [("DUSKWOODTREE01", Match::Name)]);
    assert_eq!(
        found("westfall"),
        [("BARREL01", Match::Zone), ("ElwynnGrassBase", Match::Zone)]
    );
    assert_eq!(
        found("elwynn road"),
        [("ElwynnCobblestoneBase", Match::Path)]
    );
    assert!(
        found("crate").is_empty(),
        "a zone's name counts by whole words"
    );
    assert_eq!(found("crater"), [("STREETLAMP01", Match::Zone)]);
    assert!(found("barrel duskwood").is_empty());
    assert_eq!(found("").len(), 7, "no words find everything");
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_ground_under_the_spot_comes_first_then_what_it_is_painted_with_then_the_zone_s() {
    let dir = a_catalog("ground");
    let c = Catalog::read(&dir).expect("the catalog");
    let found = Found {
        spot: Spot {
            zone: Some(ELWYNN),
            ..Spot::default()
        },
        header: String::new(),
        sheet_place: String::new(),
        under: Some(DIRT.to_ascii_uppercase()),
    };
    let (order, why) = ground_order(&c, &found);
    let names: Vec<&str> = order.iter().map(|&i| c.items[i].name()).collect();
    assert_eq!(
        names,
        ["ElwynnDirtBase", "ElwynnGrassBase", "ElwynnCobblestoneBase"]
    );
    assert_eq!(
        why[&5],
        "under the spot; Elwynn Forest paints 5.5% of its ground with it"
    );
    assert_eq!(
        why[&4],
        "60% of ElwynnDirtBase's ground lies in chunks that paint it too; \
         Elwynn Forest paints 23% of its ground with it"
    );
    let nowhere = Found {
        spot: Spot::default(),
        under: None,
        ..found
    };
    let (plain, why) = ground_order(&c, &nowhere);
    assert_eq!(
        plain,
        c.plain(Some(GROUND)),
        "without a spot, the plain order"
    );
    assert!(why.is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_list_is_a_path_a_line_with_what_to_say_after_a_tab() {
    let text =
        "# the picks for the farm\n\nWORLD\\X\\BARREL01.MDX\tby the well\n  world/y/crate.m2  \n";
    assert_eq!(
        lists::parse(text),
        vec![
            (
                "WORLD\\X\\BARREL01.MDX".to_owned(),
                "by the well".to_owned()
            ),
            ("world/y/crate.m2".to_owned(), String::new()),
        ]
    );
}

#[test]
fn a_list_written_into_the_folder_shows_and_goes_and_nothing_else_there_does() {
    let dir = std::env::temp_dir().join(format!("cairn-palette-lists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the folder");
    let mut lists = Lists::watch(Some(dir.clone()));
    std::fs::write(dir.join("farm.tmp"), format!("{BARREL}\n")).expect("a file");
    std::fs::write(dir.join(".hidden.txt"), format!("{BARREL}\n")).expect("a file");
    std::fs::write(dir.join("farm.txt"), format!("{BARREL}\tby the well\n")).expect("a list");
    let until = |lists: &mut Lists, done: &dyn Fn(&Lists) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done(lists) {
            assert!(Instant::now() < deadline, "the folder's news never came");
            lists.take_news();
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    until(&mut lists, &|l| l.all.contains_key("farm"));
    assert_eq!(lists.all.keys().collect::<Vec<_>>(), ["farm"]);
    assert_eq!(lists.all["farm"].lines[0].1, "by the well");
    assert!(
        lists.late["farm"] < Duration::from_secs(1),
        "{:?}",
        lists.late["farm"]
    );
    std::fs::remove_file(dir.join("farm.txt")).expect("the list goes");
    until(&mut lists, &|l| l.all.is_empty());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn a_damaged_picture_is_an_error() {
    let dir = a_catalog("damaged");
    let path = dir.join(format!("models/{}.png", survey::key(BARREL)));
    let shrunk = decode(&path, 96).expect("the picture");
    assert_eq!((shrunk.side, shrunk.rgba.len()), (96, 96 * 96 * 4));
    let whole = std::fs::read(&path).expect("the picture");
    let damaged = dir.join("damaged.png");
    for cut in [0, 8, 33, whole.len() / 3, whole.len() / 2] {
        std::fs::write(&damaged, &whole[..cut]).expect("a copy");
        assert!(decode(&damaged, 96).is_err(), "cut at {cut}");
    }
    std::fs::write(&damaged, &whole[..whole.len() - 1]).expect("a copy");
    let _ = decode(&damaged, 96);
    for at in [16, 40, whole.len() / 3, whole.len() - 20] {
        let mut flipped = whole.clone();
        flipped[at] ^= 0x5a;
        std::fs::write(&damaged, &flipped).expect("a copy");
        let _ = decode(&damaged, 96);
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn the_install_s_fonts_damaged_are_refused_or_drawn_never_a_panic() {
    let Some(data) = std::env::var_os("WOW_DATA") else {
        eprintln!("skipped: set WOW_DATA");
        return;
    };
    let install = world::Install::open(Path::new(&data)).expect("the install");
    let fonts = super::panel::fonts(&install).expect("the install's fonts read");
    assert_eq!(fonts.font_data.len(), 2);
    for (name, data) in &fonts.font_data {
        let whole: &[u8] = &data.font;
        let cuts = [
            0,
            12,
            100,
            whole.len() / 4,
            whole.len() / 2,
            whole.len() - 1,
        ];
        let mut copies: Vec<Vec<u8>> = cuts.iter().map(|&cut| whole[..cut].to_vec()).collect();
        for at in (0..16).map(|k| k * whole.len() / 16 + 7) {
            let mut flipped = whole.to_vec();
            flipped[at] ^= 0xff;
            copies.push(flipped);
        }
        let mut refused = 0;
        for copy in copies {
            if super::panel::font(&copy).is_err() {
                refused += 1;
                continue;
            }
            draw_text_with(copy);
        }
        assert!(
            refused >= 3,
            "{name}: only {refused} damaged copies refused"
        );
    }
}

/// Lays text out in `font` and tessellates it, as the panel does.
fn draw_text_with(font: Vec<u8>) {
    use bevy_egui::egui;
    let mut defs = egui::FontDefinitions::empty();
    let data = std::sync::Arc::new(egui::FontData::from_owned(font));
    defs.font_data.insert("f".into(), data);
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        defs.families.insert(family, vec!["f".into()]);
    }
    let ctx = egui::Context::default();
    ctx.set_fonts(defs);
    for _ in 0..2 {
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("Palette: ElwynnTreeCanopy01, 1,234 placed; 0123456789 ()[]%");
            });
        });
        let _ = ctx.tessellate(out.shapes, out.pixels_per_point);
    }
}

#[test]
fn ctrl_shift_p_opens_and_closes_the_palette_and_p_alone_does_nothing() {
    use bevy::prelude::*;
    let plugin = super::PalettePlugin {
        catalog: PathBuf::from("nowhere"),
        lists: None,
        map: crate::args::Map::Install {
            name: "Azeroth".into(),
            patch: None,
        },
    };
    let mut app = App::new();
    app.init_resource::<ButtonInput<KeyCode>>()
        .insert_resource(super::Palette::new(&plugin))
        .add_systems(Update, super::toggle);
    let mut press = |keys: &[KeyCode]| {
        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.clear();
        input.release_all();
        for key in keys {
            input.press(*key);
        }
        app.update();
        app.world().resource::<super::Palette>().open
    };
    assert!(!press(&[KeyCode::KeyP]), "P alone");
    assert!(press(&[
        KeyCode::ControlLeft,
        KeyCode::ShiftRight,
        KeyCode::KeyP
    ]));
    assert!(press(&[KeyCode::ShiftLeft]), "held open");
    assert!(!press(&[
        KeyCode::ControlRight,
        KeyCode::ShiftLeft,
        KeyCode::KeyP
    ]));
    assert!(!press(&[KeyCode::ControlLeft, KeyCode::KeyP]), "Ctrl+P");
}
