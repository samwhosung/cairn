#![allow(clippy::float_cmp)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use bevy::mesh::VertexAttributeValues;

use super::*;

#[test]
fn the_corner_reaches_farther_on_a_wider_view() {
    let fov = PerspectiveProjection::default().fov;
    let wide = corner_reach(fov, 16.0 / 9.0);
    let ultrawide = corner_reach(fov, 21.0 / 9.0);
    assert!((wide - 1.309).abs() < 0.01, "16:9 reach {wide}");
    assert!((ultrawide - 1.451).abs() < 0.01, "21:9 reach {ultrawide}");
    assert!((corner_reach(0.0, 0.0) - 1.0).abs() < 1e-6);
}

#[test]
fn the_box_distance_is_to_the_nearest_point() {
    let b = (Vec3::splat(-1.0), Vec3::splat(1.0));
    assert!(box_distance_squared(Vec3::ZERO, b).abs() < f32::EPSILON);
    assert!((box_distance_squared(Vec3::new(4.0, 0.0, 0.0), b) - 9.0).abs() < 1e-6);
    let tall = (Vec3::new(-1.0, -4.0, -1.0), Vec3::new(1.0, 4.0, 1.0));
    assert!((box_distance_squared(Vec3::new(4.0, 4.0, 0.0), tall) - 9.0).abs() < 1e-6);
}

fn hill(x0: f32, y0: f32) -> ChunkMesh {
    let cell = CHUNK_SIZE / 8.0;
    let mut positions = Vec::new();
    for row in 0..9 {
        for col in 0..9 {
            let (s, e) = (row as f32 * cell, col as f32 * cell);
            positions.push([x0 - s, y0 - e, 30.0 + s * 0.5]);
        }
        if row < 8 {
            for col in 0..8 {
                let (s, e) = ((row as f32 + 0.5) * cell, (col as f32 + 0.5) * cell);
                positions.push([x0 - s, y0 - e, 30.0 + s * 0.5]);
            }
        }
    }
    let normals = (0..VERTICES).map(|i| [i as f32, 0.0, 1.0]).collect();
    ChunkMesh {
        positions,
        normals,
        uvs: Vec::new(),
        indices: Vec::new(),
        holes: 0,
        base_texture: None,
        layer_textures: Vec::new(),
        layer_effect_ids: Vec::new(),
        alpha_map: None,
        shadow: None,
        pred_tex: [0; 64],
        no_effect_doodad: [false; 64],
        index_x: 0,
        index_y: 0,
        area_id: 0,
        impassable: false,
        liquids: Vec::new(),
    }
}

#[test]
fn the_ground_test_never_drops_a_chunk_its_box_would_take() {
    let chunk = hill(-9400.0, 60.0);
    for x in -12..=12 {
        for z in -12..=12 {
            for y in [-40.0, 45.0, 400.0] {
                let [cx, cy, _] = chunk.positions[0];
                let wow = [cx + x as f32 * 7.0, cy + z as f32 * 7.0, y];
                let eye = wow_to_bevy(wow);
                let across = footprint_distance_squared(eye, &chunk);
                let boxed = box_distance_squared(eye, bounds(&chunk));
                assert!(across <= boxed + 1e-2, "{wow:?}: {across} over {boxed}");
            }
        }
    }
}

#[test]
fn a_tuft_takes_the_normal_of_the_nearest_outer_vertex() {
    let chunk = hill(-9400.0, 60.0);
    let cell = CHUNK_SIZE / 8.0;
    let at = |row: f32, col: f32| [-9400.0 - row * cell, 60.0 - col * cell, 0.0];
    assert_eq!(ground_normal(&chunk, at(0.0, 0.0))[0], 0.0);
    assert_eq!(ground_normal(&chunk, at(2.4, 3.6))[0], (2 * 17 + 4) as f32);
    assert_eq!(ground_normal(&chunk, at(9.0, 9.0))[0], (8 * 17 + 8) as f32);
    let bare = ChunkMesh {
        normals: Vec::new(),
        ..chunk
    };
    assert_eq!(ground_normal(&bare, at(3.0, 3.0)), [0.0, 0.0, 1.0]);
}

#[test]
fn a_batch_merges_once_for_every_tuft() {
    let batch = RenderSubmesh {
        positions: vec![[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        uvs: vec![[0.0, 0.0]; 3],
        indices: vec![0, 1, 2],
        ..RenderSubmesh::default()
    };
    let tuft = |x: f32, yaw: f32| Shaded {
        tuft: Tuft {
            model: "a.m2".into(),
            position: [x, 0.0, 10.0],
            yaw,
            scale: 1.0,
        },
        tint: SHADOWED,
        ground: [0.0, 1.0, 0.0],
    };
    let (mesh, aabb) = merged(&batch, &[tuft(0.0, 0.0), tuft(5.0, 1.0)]).expect("a mesh");
    assert_eq!(mesh.count_vertices(), 6);
    let Some(Indices::U32(indices)) = mesh.indices() else {
        panic!("u32 indices");
    };
    assert_eq!(indices, &[0, 1, 2, 3, 4, 5]);
    let Some(VertexAttributeValues::Float32x4(colors)) = mesh.attribute(Mesh::ATTRIBUTE_COLOR)
    else {
        panic!("colours");
    };
    assert!(
        colors
            .iter()
            .all(|c| c[..3] == [SHADOWED; 3] && c[3] == 1.0)
    );
    assert!(aabb.min().y >= 9.99 && aabb.max().y <= 11.01);
    assert!(merged(&RenderSubmesh::default(), &[tuft(0.0, 0.0)]).is_none());
}

#[test]
fn goldshire_scatters_models_and_textures_the_install_holds() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let chain = Chain::open(data).expect("open the chain");
    let effects = Effects::read(&chain).expect("the ground effect tables");
    let tile = terrain::load_tile_mesh(&chain, "Azeroth", 31, 49).expect("Goldshire's tile");
    let tufts: Vec<Tuft> = tile
        .chunks
        .iter()
        .flat_map(|c| scatter::scatter(c, (31, 49), &effects, CELLS_PER_CHUNK))
        .collect();
    assert!(tufts.len() > 10_000, "{} tufts", tufts.len());
    let models: BTreeSet<&str> = tufts.iter().map(|t| &*t.model).collect();
    for path in &models {
        let batches = model::load_m2_mesh(&chain, path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(!batches.is_empty(), "{path} draws nothing");
        for texture in batches.iter().filter_map(|b| b.texture.as_deref()) {
            let bytes = chain
                .read(texture)
                .unwrap_or_else(|e| panic!("{texture}: {e}"));
            blp::decode_native(&bytes).unwrap_or_else(|e| panic!("{texture}: {e}"));
        }
    }
    eprintln!(
        "{} tufts of {} models: {models:?}",
        tufts.len(),
        models.len()
    );
}
