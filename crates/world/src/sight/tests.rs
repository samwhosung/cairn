use model::ModelBlend;

use super::*;

const DOWN: Vec3 = Vec3::NEG_Z;

fn flat_triangle() -> [Vec3; 3] {
    [
        Vec3::new(0.0, 0.0, -5.0),
        Vec3::new(2.0, 0.0, -5.0),
        Vec3::new(0.0, 2.0, -5.0),
    ]
}

fn met(hit: Option<Hit>) -> Option<f32> {
    hit.map(|h| h.t)
}

#[test]
fn a_face_is_met_from_its_front_and_from_behind_only_when_two_sided() {
    let up_facing = flat_triangle();
    let hit = triangle_hit(DOWN, up_facing, false).expect("met from above");
    assert!((hit.t - 5.0).abs() < 1e-6, "{}", hit.t);
    let [a, b, c] = up_facing;
    assert_eq!(met(triangle_hit(DOWN, [a, c, b], false)), None, "its back");
    assert_eq!(met(triangle_hit(DOWN, [a, c, b], true)), Some(hit.t));
    let beside = up_facing.map(|p| p + Vec3::new(1.5, 1.5, 0.0));
    assert_eq!(met(triangle_hit(DOWN, beside, true)), None, "outside it");
    assert_eq!(met(triangle_hit(Vec3::Z, up_facing, true)), None, "behind");
}

#[test]
fn a_hit_weighs_the_corners_it_lies_between() {
    let [a, b, c] = flat_triangle();
    let off = Vec3::new(-1.5, -0.25, 0.0);
    let hit = triangle_hit(DOWN, [a, b, c].map(|p| p + off), false).expect("met");
    assert!((hit.u - 0.75).abs() < 1e-6 && (hit.v - 0.125).abs() < 1e-6);
}

#[test]
fn a_box_is_entered_where_the_ray_first_crosses_it() {
    let (lo, hi) = (Vec3::splat(-1.0), Vec3::splat(1.0));
    let from = Vec3::new(-5.0, 0.0, 0.0);
    assert_eq!(slab(from, Vec3::X, lo, hi, 100.0), Some(4.0));
    assert_eq!(slab(from, Vec3::X, lo, hi, 3.0), None, "past the limit");
    assert_eq!(slab(from, Vec3::Y, lo, hi, 100.0), None, "beside it");
    assert_eq!(
        slab(Vec3::ZERO, Vec3::X, lo, hi, 100.0),
        Some(0.0),
        "inside"
    );
}

fn chunk(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> ChunkMesh {
    ChunkMesh {
        positions,
        normals: Vec::new(),
        uvs: Vec::new(),
        indices,
        holes: 0,
        base_texture: None,
        layer_textures: Vec::new(),
        layer_effect_ids: Vec::new(),
        alpha_map: None,
        shadow: None,
        pred_tex: [0; 64],
        no_effect_doodad: [false; 64],
        index_x: 3,
        index_y: 4,
        area_id: 0,
        impassable: false,
        liquids: Vec::new(),
    }
}

#[test]
fn a_chunk_is_met_on_its_faces_and_not_where_it_has_none() {
    let at = |x: f32, y: f32| [-x, -y, 10.0];
    let half = chunk(
        vec![at(0.0, 0.0), at(CHUNK_SIZE, 0.0), at(0.0, CHUNK_SIZE)],
        vec![0, 1, 2],
    );
    let above = |x: f32, y: f32| Vec3::new(-x, -y, 30.0);
    let t = chunk_hit(&half, above(1.0, 1.0), DOWN, 100.0);
    assert!(t.is_some_and(|t| (t - 20.0).abs() < 1e-4), "{t:?}");
    let across = above(CHUNK_SIZE - 1.0, CHUNK_SIZE - 1.0);
    assert_eq!(
        chunk_hit(&half, across, DOWN, 100.0),
        None,
        "the other half"
    );
    let under = Vec3::new(-1.0, -1.0, 0.0);
    assert_eq!(chunk_hit(&half, under, Vec3::Z, 100.0), None, "from below");
    assert_eq!(
        chunk_hit(&half, above(1.0, 1.0), DOWN, 19.0),
        None,
        "too far"
    );
}

/// A square of two faces a yard across, five yards below its origin, facing up.
fn square(blend: ModelBlend) -> Candidate {
    let corners = [
        [0.0, 0.0, -5.0],
        [1.0, 0.0, -5.0],
        [1.0, 1.0, -5.0],
        [0.0, 1.0, -5.0],
    ];
    Candidate {
        enters: 0.0,
        to_mesh: Affine3A::IDENTITY,
        geometry: Arc::new(RenderSubmesh {
            positions: corners.to_vec(),
            uvs: vec![[0.0, 0.0]; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
            blend,
            ..RenderSubmesh::default()
        }),
        seen: Seen::Terrain { chunk: (0, 0) },
    }
}

#[test]
fn a_batch_is_met_where_it_paints_and_seen_through_where_it_does_not() {
    let chain = Chain::default();
    let mut paints = CoverageReader::new(&chain);
    let from = wow_to_bevy([0.5, 0.25, 0.0]);
    let down = wow_to_bevy([0.0, 0.0, -1.0]);
    let opaque = batch_hit(&square(ModelBlend::Opaque), from, down, 100.0, &mut paints);
    assert!(opaque.is_some_and(|t| (t - 5.0).abs() < 1e-5), "{opaque:?}");
    assert_eq!(
        batch_hit(&square(ModelBlend::Opaque), from, down, 4.0, &mut paints),
        None
    );
    let untextured_cutout = square(ModelBlend::AlphaTest);
    assert_eq!(
        batch_hit(&untextured_cutout, from, down, 100.0, &mut paints),
        None
    );
    let modulates = square(ModelBlend::Mod);
    assert_eq!(batch_hit(&modulates, from, down, 100.0, &mut paints), None);
}
