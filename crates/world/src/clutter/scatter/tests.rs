#![allow(clippy::float_cmp)]

use terrain::{CHUNK_SIZE, ChunkMesh};

use super::*;

const CELL: f32 = CHUNK_SIZE / 8.0;
const GRASS: u32 = 7;
const BARE: u32 = 8;

/// A chunk of `tile` at `index`, its ground rising `slope` yards a yard east, with one layer of
/// ground effect `effect`.
fn chunk(tile: (u32, u32), index: (u32, u32), slope: f32, effect: u32) -> ChunkMesh {
    let x0 = (32.0 - tile.1 as f32) * terrain::TILE_SIZE - index.1 as f32 * CHUNK_SIZE;
    let y0 = (32.0 - tile.0 as f32) * terrain::TILE_SIZE - index.0 as f32 * CHUNK_SIZE;
    let mut positions = Vec::with_capacity(VERTICES);
    let ground = |south: f32, east: f32| [x0 - south, y0 - east, 40.0 + slope * east];
    for row in 0..9 {
        for col in 0..9 {
            positions.push(ground(row as f32 * CELL, col as f32 * CELL));
        }
        if row < 8 {
            for col in 0..8 {
                positions.push(ground((row as f32 + 0.5) * CELL, (col as f32 + 0.5) * CELL));
            }
        }
    }
    ChunkMesh {
        positions,
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::new(),
        holes: 0,
        base_texture: None,
        layer_textures: vec!["grass.blp".into()],
        layer_effect_ids: vec![effect],
        alpha_map: None,
        shadow: None,
        pred_tex: [0; 64],
        no_effect_doodad: [false; 64],
        index_x: index.0,
        index_y: index.1,
        area_id: 0,
        impassable: false,
        liquids: Vec::new(),
    }
}

fn effects() -> Effects {
    let model = |name: &str| Some(Arc::<str>::from(name));
    Effects(HashMap::from([(
        GRASS,
        GroundEffect {
            doodads: [model("a.m2"), model("b.m2"), None, model("a.m2")],
            density: 6,
        },
    )]))
}

fn bits(tufts: &[Tuft]) -> Vec<(String, [u32; 3], u32, u32)> {
    tufts
        .iter()
        .map(|t| {
            (
                t.model.to_string(),
                t.position.map(f32::to_bits),
                t.yaw.to_bits(),
                t.scale.to_bits(),
            )
        })
        .collect()
}

#[test]
fn the_noise_is_a_permutation() {
    let mut seen = [false; 256];
    for &b in &NOISE {
        seen[usize::from(b)] = true;
    }
    assert!(seen.iter().all(|&s| s));
}

#[test]
fn the_randomizer_repeats_and_stays_in_range() {
    let (mut a, mut b) = (Randomizer::new(0x0030_0020), Randomizer::new(0x0030_0020));
    for _ in 0..1000 {
        assert_eq!(a.next(), b.next());
    }
    assert_ne!(Randomizer::new(1).next(), Randomizer::new(2).next());
    let mut r = Randomizer::new(12345);
    for _ in 0..10_000 {
        let u = r.signed_unit();
        assert!((-1.0..=1.0).contains(&u) && u != 0.0, "{u}");
    }
}

#[test]
fn a_doodad_is_named_by_its_stem_in_the_detail_directory() {
    let path = |name: &str| detail_model(name).map(|p| p.to_string());
    assert_eq!(
        path("ElwGra01.mdl").as_deref(),
        Some("World\\NoDXT\\Detail\\ElwGra01.m2")
    );
    assert_eq!(
        path("World\\Detail\\ElwFlo01.MDX").as_deref(),
        Some("World\\NoDXT\\Detail\\ElwFlo01.m2")
    );
    assert_eq!(path(""), None);
}

#[test]
fn a_chunk_scatters_the_same_tufts_on_any_thread() {
    let ground = chunk((31, 49), (5, 9), 0.25, GRASS);
    let effects = effects();
    let here = bits(&scatter(&ground, (31, 49), &effects, 32));
    assert!(here.len() > 32, "{} tufts", here.len());
    std::thread::scope(|s| {
        let runs: Vec<_> = (0..4)
            .map(|_| s.spawn(|| bits(&scatter(&ground, (31, 49), &effects, 32))))
            .collect();
        for run in runs {
            assert_eq!(run.join().expect("the thread"), here);
        }
    });
    let elsewhere = bits(&scatter(&ground, (32, 49), &effects, 32));
    assert_ne!(elsewhere, here, "the seed is the chunk's place on the map");
}

#[test]
fn every_tuft_stands_on_the_drawn_ground_of_its_chunk() {
    let ground = chunk((31, 49), (2, 3), 0.25, GRASS);
    let [x0, y0, _] = ground.positions[0];
    let tufts = scatter(&ground, (31, 49), &effects(), 64);
    for t in &tufts {
        let [x, y, z] = t.position;
        assert!((x0 - CHUNK_SIZE..=x0).contains(&x) && (y0 - CHUNK_SIZE..=y0).contains(&y));
        let drawn = ground.height_at(t.position).expect("over the chunk");
        assert!((z - drawn).abs() < 1e-3, "{z} over ground at {drawn}");
        assert!((0.9..=1.1).contains(&t.scale) && t.yaw.abs() <= PI);
        assert_ne!(&*t.model, "", "only slots that name a model place one");
    }
    let from = |name: &str| tufts.iter().filter(|t| &*t.model == name).count();
    assert!(from("a.m2") > from("b.m2"), "two slots of four name a.m2");
}

#[test]
fn holes_bare_cells_and_layers_without_an_effect_get_nothing() {
    let effects = effects();
    let mut ground = chunk((31, 49), (0, 0), 0.0, GRASS);
    assert!(!scatter(&ground, (31, 49), &effects, 32).is_empty());
    ground.holes = u16::MAX;
    assert!(scatter(&ground, (31, 49), &effects, 32).is_empty());
    ground.holes = 0;
    ground.no_effect_doodad = [true; 64];
    assert!(scatter(&ground, (31, 49), &effects, 32).is_empty());
    let other = chunk((31, 49), (0, 0), 0.0, BARE);
    assert!(scatter(&other, (31, 49), &effects, 32).is_empty());
}
