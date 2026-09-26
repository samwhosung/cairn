use std::path::PathBuf;

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
            draw: None,
        })
    );
    assert_eq!(
        parsed("--draw 300 out/c"),
        Ok(Order {
            dir: PathBuf::from("out/c"),
            draw: Some(300),
        })
    );
    for line in ["a b", "--draw", "--draw x", "--draw 1 --draw 2", "--size 5"] {
        assert!(parsed(line).is_err(), "{line}");
    }
}

#[test]
#[ignore = "draws on the GPU; set WOW_DATA and CAIRN_PICTURES"]
fn a_lamppost_a_tree_and_a_farmhouse_stand_beside_a_player() {
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
    ]
    .iter()
    .map(|path| Sitter {
        path: (*path).to_owned(),
        building: std::path::Path::new(path)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wmo")),
        bounds: survey::bounds(&install.0, path).expect("its bounds"),
        out: dir.join(format!(
            "catalog-{}.png",
            survey::key(path).rsplit('/').next().unwrap_or("x")
        )),
    })
    .collect();
    let drawn = draw(&install, sitters, PICTURE).expect("drawn");
    for d in &drawn {
        assert!(d.failed.is_none(), "{d:?}");
    }
}
