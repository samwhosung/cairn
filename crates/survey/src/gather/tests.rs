use terrain::Doodad;

use super::*;
use crate::scan::{Listed, Underfoot};

fn tile(map: usize, at: (u32, u32), doodads: Vec<(u32, Option<u32>, &str)>) -> TileSummary {
    TileSummary {
        map,
        at,
        chunks: Vec::new(),
        doodads: doodads
            .into_iter()
            .map(|(unique_id, area, model)| Listed {
                placed: Doodad {
                    model: model.into(),
                    position: [1.0, 2.0, 3.0],
                    rotation: [0.0, 30.0, 0.0],
                    scale: 1.0,
                    unique_id,
                },
                here: area.map(|area| Underfoot {
                    area,
                    texture: None,
                    slope: None,
                }),
            })
            .collect(),
        wmos: Vec::new(),
    }
}

fn once(tiles: &[TileSummary]) -> Vec<(usize, u32, Option<u32>, &str)> {
    each_once(tiles, |t| {
        t.doodads.iter().map(|d| {
            let p = &d.placed;
            (
                d.here.as_ref(),
                p.unique_id,
                p.model.as_str(),
                p.position,
                p.rotation,
                p.scale,
                0,
            )
        })
    })
    .into_iter()
    .map(|p| (p.map, p.unique_id, p.area, p.model))
    .collect()
}

#[test]
fn a_placement_two_tiles_list_is_counted_once_from_the_tile_it_stands_on() {
    let tiles = [
        tile(0, (1, 1), vec![(7, None, "A.mdx"), (8, Some(3), "B.mdx")]),
        tile(0, (1, 2), vec![(7, Some(5), "A.mdx"), (9, None, "C.mdx")]),
        tile(1, (1, 1), vec![(7, Some(6), "D.mdx")]),
    ];
    assert_eq!(
        once(&tiles),
        vec![
            (0, 7, Some(5), "A.mdx"),
            (0, 8, Some(3), "B.mdx"),
            (0, 9, None, "C.mdx"),
            (1, 7, Some(6), "D.mdx"),
        ],
        "one id a map, from the tile that holds it"
    );
}

fn map(id: u32, directory: &str) -> MapTiles {
    MapTiles {
        id,
        directory: directory.into(),
        tiles: Vec::new(),
        global_wmo: None,
    }
}

#[test]
fn a_zone_file_is_named_by_its_zone_then_as_much_more_as_tells_it_apart() {
    let maps = [map(0, "Azeroth"), map(13, "test"), map(29, "Test")];
    let ids = [(0, 12), (0, 40), (1, 0), (2, 0), (0, 41)];
    let names: Vec<String> = [
        "Elwynn Forest",
        "Westfall",
        "(no zone)",
        "(no zone)",
        "Westfall",
    ]
    .map(str::to_owned)
    .into();
    assert_eq!(
        zone_keys(&ids, &names, &maps),
        [
            "elwynn-forest",
            "westfall-azeroth-0-40",
            "no-zone-test-13",
            "no-zone-test-29",
            "westfall-azeroth-0-41",
        ]
    );
    assert_eq!(slug("Zul'Gurub  (old)"), "zul-gurub-old");
}
