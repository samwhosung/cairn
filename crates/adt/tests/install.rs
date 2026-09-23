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

#[test]
fn a_sloped_chunks_normals_agree_with_its_heights() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = read(&chain, "World\\Maps\\Azeroth\\Azeroth_33_46.adt");
    let cell = 1600.0f32 / 3.0 / 128.0;
    let cosine = |a: [f32; 3], b: [f32; 3]| {
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let len = |v: [f32; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        f64::from(dot / (len(a) * len(b)))
    };
    let (mut vertices, mut as_read, mut z_as_y) = (0, 0.0, 0.0);
    for chunk in &tile.mcnk_chunks {
        let (Some(heights), Some(mcnr)) = (&chunk.heights, &chunk.normals) else {
            continue;
        };
        let h = |r: usize, c: usize| heights.heights[r * 17 + c];
        let (lo, hi) = heights
            .heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        if hi - lo < 20.0 {
            continue;
        }
        for r in 1..8 {
            for c in 1..8 {
                let (north, south) = (h(r - 1, c), h(r + 1, c));
                let (west, east) = (h(r, c - 1), h(r, c + 1));
                let from_heights = [
                    (south - north) / (2.0 * cell),
                    (east - west) / (2.0 * cell),
                    1.0,
                ];
                let [x, y, z] = mcnr.normals[r * 17 + c].to_normalized();
                as_read += cosine(from_heights, [x, y, z]);
                z_as_y += cosine(from_heights, [x, z, y]);
                vertices += 1;
            }
        }
    }
    assert!(vertices > 1000, "{vertices}");
    let (as_read, z_as_y) = (as_read / f64::from(vertices), z_as_y / f64::from(vertices));
    assert!(as_read > 0.98, "{as_read}");
    assert!(z_as_y < 0.5, "{z_as_y}");
}
