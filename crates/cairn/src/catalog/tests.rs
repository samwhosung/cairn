use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::studio::{Sitter, draw};
use super::*;

fn parsed(line: &str) -> Result<Order, String> {
    parse(line.split_whitespace().map(str::to_owned))
}

#[test]
fn a_catalog_goes_to_its_own_directory_unless_told() {
    assert_eq!(
        parsed(""),
        Ok(Order {
            dir: PathBuf::from("catalog"),
            draw_at_most: None,
        })
    );
    assert_eq!(
        parsed("--draw 300 out/c"),
        Ok(Order {
            dir: PathBuf::from("out/c"),
            draw_at_most: Some(300),
        })
    );
    for line in ["a b", "--draw", "--draw x", "--draw 1 --draw 2", "--size 5"] {
        assert!(parsed(line).is_err(), "{line}");
    }
}

fn model_share(png: &Path) -> f32 {
    let img = image::open(png).expect("the picture").to_rgb8();
    let near = |a: [u8; 3], b: [u8; 3], by: u8| a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= by);
    let backdrop = img.get_pixel(0, 0).0;
    let mut counts: BTreeMap<[u8; 3], u32> = BTreeMap::new();
    for p in img.pixels().filter(|p| !near(p.0, backdrop, 6)) {
        *counts.entry(p.0).or_default() += 1;
    }
    let figure = counts
        .iter()
        .max_by_key(|(_, n)| **n)
        .map_or(backdrop, |(c, _)| *c);
    let model = img
        .pixels()
        .filter(|p| !near(p.0, backdrop, 12) && !near(p.0, figure, 2))
        .count();
    model as f32 / (img.width() * img.height()) as f32
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_lamppost_a_tree_an_inn_and_a_dungeon_stand_beside_a_player() {
    let (Some(data), Some(dir)) = (
        std::env::var_os("WOW_DATA"),
        std::env::var_os("CAIRN_PICTURES"),
    ) else {
        eprintln!("skipped: set WOW_DATA and CAIRN_PICTURES");
        return;
    };
    let install = world::Install::open(&PathBuf::from(data)).expect("the install");
    let dir = PathBuf::from(dir);
    let sitters: Vec<Sitter> = [
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\LampPost\\LampPost.mdx",
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Trees\\ElwynnTreeMid01.mdx",
        "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo",
        "World\\wmo\\Dungeon\\AZ_StormwindPrisons\\StormwindPrison.wmo",
    ]
    .iter()
    .map(|path| Sitter {
        install_path: (*path).to_owned(),
        building: Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wmo")),
        bounds: survey::bounds(&install.0, path).expect("its bounds"),
        out: dir.join(format!(
            "catalog-{}.png",
            survey::key(path).rsplit('/').next().unwrap_or("x")
        )),
    })
    .collect();
    let outs: Vec<PathBuf> = sitters.iter().map(|s| s.out.clone()).collect();
    let drawn = draw(&install, sitters, survey::PICTURE_SIDE).expect("drawn");
    for (d, out) in drawn.iter().zip(&outs) {
        assert!(d.trouble.is_none(), "{d:?}");
        let share = model_share(out);
        assert!(share > 0.02, "{}: {share}", d.install_path);
    }
}
