#![allow(clippy::float_cmp)]

use std::io::Cursor;
use std::path::PathBuf;

use mpq::Chain;
use wdt::{WdtFile, WdtReader};

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn read(chain: &Chain, name: &str) -> WdtFile {
    let bytes = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
    WdtReader::new(Cursor::new(bytes))
        .read()
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn tile_count(wdt: &WdtFile) -> usize {
    (0..64)
        .flat_map(|y| (0..64).map(move |x| (x, y)))
        .filter(|&(x, y)| wdt.get_tile(x, y).is_some_and(|t| t.has_adt))
        .count()
}

#[test]
fn every_map_parses_and_twenty_are_one_building() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let names: Vec<String> = chain
        .list()
        .into_iter()
        .map(|entry| entry.name)
        .filter(|name| name.to_ascii_lowercase().ends_with(".wdt"))
        .collect();
    assert_eq!(names.len(), 43);
    let terrainless: Vec<WdtFile> = names
        .iter()
        .map(|name| read(&chain, name))
        .filter(|wdt| wdt.global_wmo().is_some())
        .collect();
    assert_eq!(terrainless.len(), 20);
    for wdt in &terrainless {
        assert_eq!(tile_count(wdt), 0);
        assert_eq!(wdt.global_wmo().map(|g| g.position), Some([0.0; 3]));
    }
}

#[test]
fn reads_the_continents() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let azeroth = read(&chain, "World\\Maps\\Azeroth\\Azeroth.wdt");
    assert_eq!(tile_count(&azeroth), 687);
    assert!(azeroth.global_wmo().is_none());
    let (x, y) = wdt::world_to_tile(-8949.95, -132.493);
    assert!(
        azeroth
            .get_tile(x as usize, y as usize)
            .is_some_and(|t| t.has_adt)
    );
    assert_eq!(
        tile_count(&read(&chain, "World\\Maps\\Kalimdor\\Kalimdor.wdt")),
        1018
    );
}

#[test]
fn reads_dire_maul_turned_around() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let dire_maul = read(&chain, "World\\Maps\\DireMaul\\DireMaul.wdt");
    let g = dire_maul.global_wmo().expect("a terrainless map");
    assert_eq!(
        g.model,
        "world\\wmo\\dungeon\\kl_diremaul\\kl_diremaul_instance.wmo"
    );
    assert_eq!(g.rotation, [0.0, 180.0, 0.0]);
}
