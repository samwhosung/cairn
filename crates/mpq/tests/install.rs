use std::collections::HashSet;
use std::path::{Path, PathBuf};

use mpq::{Chain, ChainError};

fn wow_data_or_skip() -> Option<PathBuf> {
    let data = std::env::var_os("WOW_DATA").map(PathBuf::from);
    if data.is_none() {
        eprintln!("skipped: WOW_DATA is not set");
    }
    data
}

#[test]
fn reads_spell_dbc() {
    let Some(data) = wow_data_or_skip() else {
        return;
    };
    let chain = Chain::open(data).expect("open the chain");
    let bytes = chain
        .read("DBFilesClient/Spell.dbc")
        .expect("read Spell.dbc");
    assert_eq!(&bytes[..4], b"WDBC");
    assert!(
        bytes.len() > 1_000_000,
        "Spell.dbc is {} bytes",
        bytes.len()
    );
}

#[test]
fn reads_across_archives() {
    let Some(data) = wow_data_or_skip() else {
        return;
    };
    let chain = Chain::open(data).expect("open the chain");
    for (name, archive) in [
        ("DBFilesClient/Spell.dbc", "patch-2.MPQ"),
        ("DBFilesClient/TaxiNodes.dbc", "patch-2.MPQ"),
        (
            "Interface/Icons/Spell_Holy_ArcaneIntellect.blp",
            "patch.MPQ",
        ),
        ("Creature\\Kobold\\Kobold.m2", "model.MPQ"),
    ] {
        assert!(chain.contains(name), "{name}");
        assert_eq!(
            chain.archive_of(name).and_then(Path::file_name),
            Some(archive.as_ref()),
            "{name}"
        );
        let bytes = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
        assert!(!bytes.is_empty(), "{name} is empty");
    }
}

#[test]
fn delete_markers_hide_the_base_copy() {
    let Some(data) = wow_data_or_skip() else {
        return;
    };
    let base = Chain::open(data.join("model.MPQ")).expect("open model.MPQ");
    let chain = Chain::open(&data).expect("open the chain");
    let listed: HashSet<String> = chain
        .list()
        .into_iter()
        .map(|entry| entry.name.replace('/', "\\").to_ascii_lowercase())
        .collect();
    for name in [
        "Creature\\OgreMage\\OgreMage.m2",
        "Creature\\OgreWarlord\\OgreWarlord.m2",
    ] {
        let live = base.read(name).unwrap_or_else(|e| panic!("{e}"));
        assert!(live.len() > 100_000, "{name} is {} bytes", live.len());
        assert!(!chain.contains(name), "{name}");
        assert!(
            matches!(chain.read(name), Err(ChainError::Deleted { .. })),
            "{name}"
        );
        assert!(!listed.contains(&name.to_ascii_lowercase()), "{name}");
    }
}

#[test]
fn a_patch_directory_is_read_in_place_of_the_install() {
    let Some(data) = wow_data_or_skip() else {
        return;
    };
    let dir = std::env::temp_dir().join(format!("mpq-install-patch-{}", std::process::id()));
    let maps = dir.join("world/MAPS/azeroth");
    std::fs::create_dir_all(&maps).expect("make the patch");
    std::fs::write(maps.join("AZEROTH_32_48.ADT"), b"patched").expect("write the patch");
    let install = Chain::open(&data).expect("open the chain");
    let patched = Chain::open(&data)
        .and_then(|chain| chain.with_patch(&dir))
        .expect("lay the patch");
    let (tile, beside) = (
        "World\\Maps\\Azeroth\\Azeroth_32_48.adt",
        "World\\Maps\\Azeroth\\Azeroth_32_49.adt",
    );
    assert_eq!(patched.read(tile).expect("the patched tile"), b"patched");
    let original = install.read(tile).expect("the install's tile");
    assert_eq!(&original[..4], b"REVM");
    let unpatched = install.read(beside).expect("the install's tile beside");
    assert_eq!(patched.read(beside).expect("the tile beside"), unpatched);
    std::fs::remove_dir_all(&dir).expect("clean up");
}
