use std::path::PathBuf;

use mpq::Chain;
use wdl::WdlFile;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn wdl(chain: &Chain, map: &str) -> WdlFile {
    let bytes = chain
        .read(&format!("World\\Maps\\{map}\\{map}.wdl"))
        .unwrap_or_else(|e| panic!("{map}: {e}"));
    WdlFile::parse(&bytes).unwrap_or_else(|e| panic!("{map}: {e}"))
}

#[test]
fn the_continents_parse() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for map in ["Azeroth", "Kalimdor"] {
        let wdl = wdl(&chain, map);
        let present = wdl.present_count();
        assert!(present > 500, "{map}: {present} tiles");
        let ring = wdl.tiles_around(-8949.95, -132.49, 5);
        assert!(ring.len() <= 121, "{map}: {}", ring.len());
        for (x, y) in ring {
            let mesh = wdl.tile_mesh(x, y).expect("a ring tile is present");
            assert_eq!((mesh.positions.len(), mesh.indices.len()), (545, 3072));
        }
    }
}

#[test]
fn chunk_corners_sit_on_the_terrain_they_summarise() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let wdl = wdl(&chain, "Azeroth");
    for (x, y) in [(32, 48), (31, 49), (38, 27)] {
        let tile = terrain::load_tile_mesh(&chain, "Azeroth", x, y).expect("the tile loads");
        let mesh = wdl.tile_mesh(x, y).expect("the tile has heights");
        let mut worst = 0.0f32;
        for chunk in &tile.chunks {
            let corner = chunk.positions[0];
            let at = (chunk.index_y * 17 + chunk.index_x) as usize;
            let coarse = mesh.positions[at];
            assert!(
                (coarse[0] - corner[0]).abs() < 0.01,
                "{x}_{y}: {coarse:?} {corner:?}"
            );
            assert!(
                (coarse[1] - corner[1]).abs() < 0.01,
                "{x}_{y}: {coarse:?} {corner:?}"
            );
            worst = worst.max((coarse[2] - corner[2]).abs());
        }
        assert!(
            worst < 1.0,
            "{x}_{y}: a corner is {worst} yd off the terrain"
        );
    }
}
