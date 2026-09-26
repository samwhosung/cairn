//! What a ground texture is, told by the words in its file name: a guess its swatch confirms.

/// The kinds, each with the words that name it, first match wins: a rock road is a road and a
/// rocky mud is mud.
const GROUND_KINDS: [(&str, &[&str]); 12] = [
    (
        "road",
        &["road", "cobble", "brick", "path", "tile", "floor"],
    ),
    ("snow", &["snow", "ice"]),
    ("lava", &["lava", "magma"]),
    ("slime", &["slime"]),
    (
        "seabed",
        &["underwater", "oceanfloor", "seaweed", "barnacle"],
    ),
    ("sand", &["sand", "beach", "shore", "saltflat"]),
    ("mud", &["mud", "muck", "swamp"]),
    ("crop", &["crop", "straw", "field"]),
    ("ash", &["ash", "charcoal", "black", "burn"]),
    (
        "rock",
        &[
            "rock", "stone", "gravel", "rubble", "shale", "cliff", "pebble",
        ],
    ),
    (
        "grass",
        &[
            "grass", "flower", "fern", "brush", "bush", "scrub", "weed", "plant", "vine", "moss",
            "root", "leaf", "needle", "creep", "web",
        ],
    ),
    ("dirt", &["dirt", "ground", "earth", "footprint", "crack"]),
];

/// The kind of the ground texture at `path`, `other` when no word names one.
pub(crate) fn ground_kind(path: &str) -> &'static str {
    let name = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    GROUND_KINDS
        .iter()
        .find(|(_, words)| words.iter().any(|w| name.contains(w)))
        .map_or("other", |(kind, _)| kind)
}

/// Every ground kind, in the order the catalog lists them.
pub(crate) fn ground_kinds() -> impl Iterator<Item = &'static str> {
    GROUND_KINDS
        .iter()
        .map(|(kind, _)| *kind)
        .chain(std::iter::once("other"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ground_is_named_by_the_first_kind_its_name_holds() {
        for (path, want) in [
            ("Tileset\\Elwynn\\ElwynnGrassBase.blp", "grass"),
            ("Tileset\\Elwynn\\ElwynnCobbleStoneBase.blp", "road"),
            ("Tileset\\Aerie\\AeriePeaksRockRoadBase.blp", "road"),
            ("Tileset\\Loch\\LochModanRockyMudBase02.blp", "mud"),
            ("Tileset\\Winter\\WinterspringRockSnow.blp", "snow"),
            ("Tileset\\Westfall\\WestFallSandGrassBase.blp", "sand"),
            ("Tileset\\Duskwood\\DuskwoodDirt.blp", "dirt"),
            ("Tileset\\Generic\\Checkers.blp", "other"),
        ] {
            assert_eq!(ground_kind(path), want, "{path}");
        }
        assert!(ground_kinds().any(|k| k == "other"));
    }
}
