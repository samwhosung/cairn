use std::collections::HashSet;
use std::path::PathBuf;

use adt::{CombinedAlphaMap, RootAdt, parse_adt};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn read(chain: &Chain, name: &str) -> RootAdt {
    let bytes = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
    parse_adt(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn a_tile_is_a_full_grid_whose_indices_resolve() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = read(&chain, "World\\Maps\\Azeroth\\Azeroth_32_48.adt");
    assert_eq!(tile.mcnk_chunks.len(), 256);
    let cells: HashSet<(u32, u32)> = tile
        .mcnk_chunks
        .iter()
        .map(|c| (c.header.index_x, c.header.index_y))
        .collect();
    assert_eq!(cells.len(), 256);
    assert!(cells.iter().all(|&(x, y)| x < 16 && y < 16));
    for chunk in &tile.mcnk_chunks {
        assert_eq!(chunk.heights.as_ref().map(|h| h.heights.len()), Some(145));
        assert_eq!(chunk.normals.as_ref().map(|n| n.normals.len()), Some(145));
        let layers = &chunk.layers.as_ref().expect("MCLY").layers;
        assert!((1..=4).contains(&layers.len()));
        assert!(
            layers
                .iter()
                .all(|l| (l.texture_id as usize) < tile.textures.len())
        );
    }
    assert!(
        tile.doodad_placements
            .iter()
            .all(|d| (d.name_id as usize) < tile.models.len())
    );
    assert!(
        tile.wmo_placements
            .iter()
            .all(|w| (w.name_id as usize) < tile.wmos.len())
    );
    assert!(
        tile.models
            .iter()
            .all(|m| m.to_ascii_lowercase().ends_with(".mdx"))
    );
}

#[test]
fn blends_the_layers_of_a_chunk() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = read(&chain, "World\\Maps\\Azeroth\\Azeroth_32_48.adt");
    let chunk = tile
        .mcnk_chunks
        .iter()
        .find(|c| c.layers.as_ref().is_some_and(|l| l.layers.len() >= 3))
        .expect("a chunk with three layers");
    let rgba = CombinedAlphaMap::new(chunk, false, true);
    let pixels = rgba.as_slice().as_chunks::<4>().0;
    assert_eq!(pixels.len(), 64 * 64);
    assert!(pixels.iter().all(|p| p[3] == 255));
    assert!(pixels.iter().any(|p| p[0] > 0));
    assert!(pixels.iter().any(|p| p[1] > 0));
}

#[test]
fn searing_gorge_is_walled_off() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = read(&chain, "World\\Maps\\Azeroth\\Azeroth_33_44.adt");
    let walled = tile
        .mcnk_chunks
        .iter()
        .filter(|c| c.header.impassable())
        .count();
    assert_eq!(walled, 48);
}

#[test]
fn a_river_mouth_carries_two_liquids() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = read(&chain, "World\\Maps\\Azeroth\\Azeroth_33_38.adt");
    let mouths: Vec<_> = tile
        .mcnk_chunks
        .iter()
        .filter(|c| c.liquids.len() == 2)
        .collect();
    assert_eq!(mouths.len(), 3);
    for chunk in mouths {
        assert_eq!((chunk.header.flags & 0x3c).count_ones(), 2);
        let [a, b] = &chunk.liquids[..] else {
            unreachable!()
        };
        assert_eq!(a.vertices.len(), 81);
        assert_ne!(a.tile_flags, b.tile_flags);
    }
}
