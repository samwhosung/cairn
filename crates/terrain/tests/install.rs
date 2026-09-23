use std::path::PathBuf;

use mpq::Chain;
use terrain::{
    ChunkMesh, Error, LiquidKind, LiquidMesh, MapTiles, TILE_SIZE, TileMesh, build_liquid_mesh,
    find_tile_near, impassable_at, load_tile_mesh, mcsh_shadowed_at, terrain_height_at,
    triangle_z_at,
};

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn tile(chain: &Chain, x: u32, y: u32) -> TileMesh {
    load_tile_mesh(chain, "Azeroth", x, y).unwrap_or_else(|e| panic!("Azeroth_{x}_{y}: {e}"))
}

fn liquids(tile: &TileMesh) -> impl Iterator<Item = &LiquidMesh> {
    tile.chunks.iter().flat_map(|c| &c.liquids)
}

#[test]
fn maps_split_into_terrain_and_a_single_building() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let whole_map = |map: &str| {
        let tiles = MapTiles::load(&chain, map).expect(map);
        assert_eq!(tiles.map(), map);
        tiles.existing_in_radius(0.0, 0.0, 32).len()
    };
    assert_eq!(whole_map("StormwindJail"), 0);
    assert!(matches!(
        find_tile_near(&chain, "StormwindJail", 0.0, 0.0),
        Err(Error::NoTileNear { .. })
    ));
    assert_eq!(whole_map("DeadminesInstance"), 36);
    assert!(matches!(
        MapTiles::load(&chain, "NoSuchMap"),
        Err(Error::Read { .. })
    ));
}

#[test]
fn an_impassable_band_starts_one_chunk_east_across_a_tile_seam() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let west = [-6601.98, -531.87, 335.60];
    let east = [-6601.98, -535.0, 335.60];
    let (here, next) = (tile(&chain, 32, 44), tile(&chain, 33, 44));
    assert_eq!(impassable_at(&here.chunks, west), Some(false));
    assert_eq!(impassable_at(&next.chunks, east), Some(true));
    assert_eq!(impassable_at(&here.chunks, east), None);
    assert_eq!(impassable_at(&next.chunks, west), None);
    assert_eq!(next.chunks.iter().filter(|c| c.impassable).count(), 48);
    assert_eq!(here.chunks.iter().filter(|c| c.impassable).count(), 0);
}

#[test]
fn the_chunk_lookup_answers_like_walking_every_triangle() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let walk = |chunks: &[ChunkMesh], p: [f32; 3]| {
        chunks.iter().find_map(|c| {
            c.indices.as_chunks::<3>().0.iter().find_map(|t| {
                let tri = t.map(|i| c.positions[i as usize]);
                triangle_z_at(&tri, p[0], p[1])
            })
        })
    };
    let mut hits = 0;
    for (x, y) in [(32, 48), (31, 48), (30, 48), (31, 47)] {
        let tile = tile(&chain, x, y);
        assert_eq!(tile.chunks.len(), 256);
        let nw = tile.chunks[0].positions[0];
        let step = TILE_SIZE / 64.0;
        for i in 0..64 {
            for j in 0..64 {
                let p = [
                    nw[0] - (i as f32 + 0.37) * step,
                    nw[1] - (j as f32 + 0.61) * step,
                    0.0,
                ];
                let h = walk(&tile.chunks, p);
                assert_eq!(terrain_height_at(&tile.chunks, p), h, "height at {p:?}");
                let s = tile.chunks.iter().find_map(|c| c.mcsh_shadowed_at(p));
                assert_eq!(mcsh_shadowed_at(&tile.chunks, p), s, "shadow at {p:?}");
                hits += usize::from(h.is_some());
            }
        }
    }
    assert!(hits > 12000, "{hits}");
}

#[test]
fn burning_steppes_lava_is_magma_with_authored_uvs() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = tile(&chain, 33, 46);
    let magma: Vec<&LiquidMesh> = liquids(&tile)
        .filter(|m| m.kind == LiquidKind::Magma)
        .collect();
    assert_eq!(magma.len(), 64);
    assert!(
        magma
            .iter()
            .all(|m| m.sound_nibble == 6 && m.kind.is_fullbright())
    );
    assert!(liquids(&tile).all(|m| m.kind != LiquidKind::Slime));
    assert!(magma.iter().any(|m| {
        let du = (m.uvs[1][0] - m.uvs[0][0]).abs();
        du > 0.001 && (du - 0.25).abs() > 0.05
    }));
}

#[test]
fn a_river_mouth_chunk_builds_its_river_and_its_sea() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = chain
        .read("World\\Maps\\Azeroth\\Azeroth_33_33.adt")
        .expect("read");
    let root = adt::parse_adt(&bytes).expect("parse");
    let mut mouths = 0;
    for mcnk in root.mcnk_chunks.iter().filter(|m| m.liquids.len() > 1) {
        mouths += 1;
        let built: Vec<LiquidMesh> = mcnk
            .liquids
            .iter()
            .filter_map(|lq| build_liquid_mesh(lq, mcnk.header.position))
            .collect();
        assert_eq!(built.len(), 2);
        assert_eq!(
            (built[0].kind, built[1].kind),
            (LiquidKind::Still, LiquidKind::Ocean)
        );
        let surface = |m: &LiquidMesh| m.positions[m.indices[0] as usize][2];
        assert!(surface(&built[0]) > surface(&built[1]) + 1.0);
        assert!(
            built[0]
                .wet
                .iter()
                .zip(&built[1].wet)
                .all(|(r, o)| !(*r && *o))
        );
    }
    assert!(mouths > 0);
}

#[test]
fn ocean_and_river_depths_ride_their_own_divisors() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for (x, y, kind, divisor) in [
        (41, 41, LiquidKind::Ocean, 255.0),
        (32, 48, LiquidKind::Still, 42.0),
    ] {
        let bytes = chain
            .read(&format!("World\\Maps\\Azeroth\\Azeroth_{x}_{y}.adt"))
            .expect("read");
        let root = adt::parse_adt(&bytes).expect("parse");
        let mut seen = 0;
        for mcnk in &root.mcnk_chunks {
            for block in &mcnk.liquids {
                let Some(mesh) = build_liquid_mesh(block, mcnk.header.position) else {
                    continue;
                };
                if mesh.kind != kind {
                    continue;
                }
                for (v, d) in block.vertices.iter().zip(&mesh.depths) {
                    let want = (f32::from(v.depth_byte()) / divisor).clamp(0.0, 1.0);
                    assert_eq!(d.to_bits(), want.to_bits());
                    seen += 1;
                }
            }
        }
        assert!(seen > 0, "{kind:?} on Azeroth_{x}_{y}");
    }
}

#[test]
fn flag_0x8000_reads_alpha_maps_whole() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let mut bytes = chain
        .read("World\\Maps\\Azeroth\\Azeroth_32_48.adt")
        .expect("read");
    let edge_fixed = terrain::adt_to_tile_mesh(&bytes).expect("mesh");
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("4 bytes"));
        if &bytes[at..at + 4] == b"KNCM" {
            bytes[at + 9] |= 0x80;
        }
        at += 8 + size as usize;
    }
    let whole = terrain::adt_to_tile_mesh(&bytes).expect("mesh");
    let root = adt::parse_adt(&bytes).expect("parse");
    let mut changed = 0;
    for ((mcnk, a), b) in root
        .mcnk_chunks
        .iter()
        .zip(&edge_fixed.chunks)
        .zip(&whole.chunks)
    {
        assert_ne!(mcnk.header.flags & adt::MCNK_DO_NOT_FIX_ALPHA, 0);
        let (Some(a), Some(b)) = (&a.alpha_map, &b.alpha_map) else {
            continue;
        };
        let as_read = adt::CombinedAlphaMap::new(mcnk, false, false);
        assert_eq!(b.as_slice(), as_read.as_slice());
        changed += usize::from(a != b);
    }
    assert!(changed > 0);
}

#[test]
fn elwynns_lake_tile_is_still_water_on_a_sane_grid() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let tile = tile(&chain, 32, 48);
    let meshes: Vec<&LiquidMesh> = liquids(&tile).collect();
    assert_eq!(meshes.len(), 42);
    for m in meshes {
        assert_eq!(m.kind, LiquidKind::Still);
        assert_eq!(
            (m.positions.len(), m.uvs.len(), m.depths.len()),
            (81, 81, 81)
        );
        assert!(m.positions.iter().all(|p| p[2].abs() < 10_000.0));
        assert_eq!(m.indices.len() % 6, 0);
        assert!(m.indices.iter().all(|&i| {
            let z = m.positions[i as usize][2];
            (50.0..300.0).contains(&z)
        }));
    }
}
