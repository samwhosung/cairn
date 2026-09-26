use std::collections::BTreeSet;
use std::path::PathBuf;

use atlas::{Areas, Doodads, frame, mark, render, texture_colors};
use mpq::Chain;
use terrain::load_tile_mesh;

const YPP: f32 = 4.0;
const GOLDSHIRE_INN: [f32; 2] = [-9464.2, 24.4];
const CRYSTAL_LAKE_DEEPEST: [f32; 2] = [-9450.0, -116.7];

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

#[test]
fn goldshire_and_crystal_lake_are_drawn_from_two_tiles() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let areas = Areas::load(&chain).expect("AreaTable");
    let elwynn = areas
        .zone_named("elwynn forest", None)
        .expect("Elwynn Forest");
    assert_eq!(areas.get(elwynn).map(|a| a.map), Some(0), "on Azeroth");
    let westfall = |map| {
        let id = areas.zone_named("Westfall", map)?;
        areas.get(id).map(|a| a.map)
    };
    assert_eq!(
        westfall(None),
        Some(0),
        "the open world's before an instance's"
    );
    assert_eq!(westfall(Some(36)), Some(36));
    let loaded: Vec<_> = [(31, 49), (32, 49)]
        .map(|(x, y)| {
            (
                (x, y),
                load_tile_mesh(&chain, "Azeroth", x, y).expect("a tile"),
            )
        })
        .into();
    let f = frame(&loaded, &areas, Some(elwynn)).expect("Elwynn is on the tiles");
    assert_eq!((f.x0, f.x1, f.y0, f.y1), (31, 32, 49, 49));
    let named: BTreeSet<&String> = loaded
        .iter()
        .flat_map(|(_, t)| t.chunks.iter().flat_map(|c| &c.layer_textures))
        .collect();
    let colors = texture_colors(&chain, &loaded);
    assert_eq!(colors.len(), named.len(), "every texture decodes");

    let (mut img, counts) = render(&loaded, &colors, &areas, Some(elwynn), &f, YPP).expect("draws");
    let (again, _) = render(&loaded, &colors, &areas, Some(elwynn), &f, YPP).expect("draws");
    assert!(img == again, "the same map twice");
    assert_eq!(img.dimensions(), (266, 133));
    let drawn = Doodads {
        trees: 493,
        shrubs: 1106,
        rocks: 79,
        fences: 480,
        props: 460,
    };
    assert_eq!(counts, drawn);
    let greyed = img
        .pixels()
        .filter(|p| p.0[0] == p.0[1] && p.0[2] > p.0[0])
        .count();
    assert_eq!(
        greyed, 334,
        "the five chunks of Stormwind City, less their marks"
    );
    let [col, row] = f.pixel(YPP, GOLDSHIRE_INN);
    assert_eq!(img.get_pixel(col as u32 + 3, row as u32).0, [200, 30, 30]);
    let sum = img.pixels().fold([0u64; 3], |s, p| {
        [0, 1, 2].map(|c| s[c] + u64::from(p.0[c]))
    });
    let mean = sum.map(|s| s / u64::from(img.width() * img.height()));
    assert_eq!(mean, [86, 87, 23], "olive, in the textures' colours");
    let [col, row] = f.pixel(YPP, CRYSTAL_LAKE_DEEPEST);
    let [r, g, b] = img.get_pixel(col as u32, row as u32).0;
    assert!(b > g && g > r, "fourteen yards of water: {r} {g} {b}");

    assert!(mark(&mut img, &f, YPP, GOLDSHIRE_INN));
    assert!(!mark(&mut img, &f, YPP, [-8000.0, 24.4]), "a tile north");
}
