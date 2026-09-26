use std::collections::BTreeMap;
use std::path::PathBuf;

use super::*;

#[test]
fn a_path_is_keyed_in_lowercase_without_its_extension() {
    assert_eq!(
        key("World\\Generic\\Human\\Passive Doodads\\Barrel\\Barrel01.MDX"),
        "world/generic/human/passive_doodads/barrel/barrel01"
    );
    assert_eq!(
        key("Tileset\\Elwynn\\ElwynnGrassBase.blp"),
        "tileset/elwynn/elwynngrassbase"
    );
    assert_eq!(key("a.b\\c"), "a.b/c", "a dot in a directory stays");
    assert_eq!(key("World\\wmo\\Farm.wmo"), key("WORLD\\WMO\\farm.WMO"));
}

#[test]
fn the_spelling_used_most_names_a_file() {
    let counts: BTreeMap<String, u32> = [("A.mdx", 2), ("a.mdx", 5), ("A.MDX", 5)]
        .map(|(s, n)| (s.to_owned(), n))
        .into();
    assert_eq!(
        spelling(&counts),
        "A.MDX",
        "the least in byte order on a tie"
    );
    assert_eq!(spelling(&BTreeMap::new()), "");
}

#[test]
fn the_maps_are_read_whole_each_placement_once() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let chain = Chain::open(data).expect("the install");
    let inv = read(&chain).expect("read");
    let on_ground: u32 = inv
        .models
        .iter()
        .filter(|m| !m.building)
        .map(|m| m.on_ground)
        .sum();
    assert_eq!(on_ground, 378_639, "every doodad the maps place, once");
    assert!(inv.models.iter().all(|m| m.bounds.is_some()));
    let mut keys: Vec<&str> = inv.zones.iter().map(|z| z.key.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), inv.zones.len(), "a file a zone");
    let elwynn = inv
        .zones
        .iter()
        .find(|z| z.name == "Elwynn Forest")
        .expect("Elwynn");
    let census = atlas::Doodads {
        trees: 2121,
        shrubs: 4723,
        rocks: 666,
        fences: 1347,
        props: 1788,
    };
    assert_eq!(elwynn.doodads, census, "as atlas counts it");
    let lamp = inv
        .models
        .iter()
        .find(|m| m.key == "world/azeroth/elwynn/passivedoodads/lamppost/lamppost")
        .expect("Goldshire's lamp post");
    let [lo, hi] = lamp.bounds.expect("its box");
    assert!((hi[2] - lo[2] - 4.1).abs() < 0.01, "4.1 yd tall");
    assert_eq!(
        lamp.zones.first().map(|z| inv.zones[z.index].name.as_str()),
        Some("Elwynn Forest")
    );
    let grass = inv
        .grounds
        .iter()
        .find(|g| g.key == "tileset/elwynn/elwynngrassbase")
        .expect("Elwynn's grass");
    assert_eq!(grass.kind, "grass");
    assert_eq!(
        grass.zones.first().map(|z| inv.zones[z.0].name.as_str()),
        Some("Elwynn Forest")
    );
}
