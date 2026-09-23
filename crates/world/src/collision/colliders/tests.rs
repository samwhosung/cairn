use std::path::PathBuf;

use avian3d::prelude::PhysicsPlugins;
use bevy::app::TaskPoolPlugin;
use bevy::ecs::system::RunSystemOnce;
use terrain::{CHUNK_SIZE, TILE_SIZE};

use super::*;
use crate::collision::tests::physics_app;
use crate::collision::{WorldCollision, walk_layers};

fn chunk(col: u32, row: u32, impassable: bool) -> ChunkMesh {
    let (nw_x, nw_y) = (-(row as f32) * CHUNK_SIZE, -(col as f32) * CHUNK_SIZE);
    let mut positions = vec![[0.0_f32; 3]; 145];
    for r in 0..9usize {
        for c in 0..9usize {
            positions[r * 17 + c] = [
                nw_x - r as f32 * CHUNK_SIZE / 8.0,
                nw_y - c as f32 * CHUNK_SIZE / 8.0,
                0.0,
            ];
        }
    }
    ChunkMesh {
        positions,
        normals: Vec::new(),
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
        index_x: col,
        index_y: row,
        area_id: 0,
        impassable,
        liquids: Vec::new(),
    }
}

fn chunk_centre(col: u32, row: u32) -> Vec3 {
    wow_to_bevy([
        -(row as f32 + 0.5) * CHUNK_SIZE,
        -(col as f32 + 0.5) * CHUNK_SIZE,
        0.0,
    ])
}

fn normal(verts: &[Vec3], t: &[u32; 3]) -> Vec3 {
    let (a, b, c) = (
        verts[t[0] as usize],
        verts[t[1] as usize],
        verts[t[2] as usize],
    );
    (b - a).cross(c - a).normalize()
}

#[test]
fn a_flagged_chunk_is_boxed_in_by_outward_facing_walls() {
    let chunks: Vec<ChunkMesh> = [(1, 1, true), (0, 1, false), (1, 0, false)]
        .into_iter()
        .map(|(col, row, f)| chunk(col, row, f))
        .collect();
    let (verts, tris) = impassable_wall_data(&chunks).expect("four walls");
    assert_eq!(tris.len(), 8);
    let centre = chunk_centre(1, 1);
    for t in &tris {
        let n = normal(&verts, t);
        assert!(n.y.abs() < 1e-5, "a wall face is vertical: {n:?}");
        let face = (verts[t[0] as usize] + verts[t[1] as usize] + verts[t[2] as usize]) / 3.0;
        assert!(n.dot(face - centre) > 0.0, "{face:?} faces into the chunk");
    }
}

#[test]
fn every_flagged_chunk_fences_all_four_of_its_own_sides() {
    let chunks: Vec<ChunkMesh> = [(1, 1, true), (2, 1, true), (0, 1, false), (3, 1, false)]
        .into_iter()
        .map(|(col, row, f)| chunk(col, row, f))
        .collect();
    let (verts, tris) = impassable_wall_data(&chunks).expect("two flagged chunks");
    assert_eq!(tris.len(), 16);
    let shared = 2.0 * CHUNK_SIZE;
    let normals: Vec<Vec3> = tris
        .iter()
        .filter(|t| {
            t.iter()
                .all(|&i| (verts[i as usize].x - shared).abs() < 1e-3)
        })
        .map(|t| normal(&verts, t))
        .collect();
    assert_eq!(normals.len(), 4);
    assert!(normals.iter().any(|n| n.x > 0.9) && normals.iter().any(|n| n.x < -0.9));
}

#[test]
fn a_fence_stands_on_the_chunk_floor_and_only_rises() {
    let mut c = chunk(1, 1, true);
    c.positions[4 * 17 + 4][2] = -12.0;
    let (verts, _) = impassable_wall_data(&[c]).expect("walls");
    let (lo, hi) = verts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| {
        (lo.min(v.y), hi.max(v.y))
    });
    assert!((lo + 12.0).abs() < 1e-3, "based at the floor, got {lo}");
    assert!((hi - (FENCE_REACH - 12.0)).abs() < 1e-3, "{hi}");
}

#[test]
fn a_tile_with_no_flagged_chunk_builds_no_wall() {
    assert!(impassable_wall_data(&[chunk(0, 0, false), chunk(1, 0, false)]).is_none());
}

fn grid(chunks: usize) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let (mut verts, mut tris) = (Vec::new(), Vec::new());
    for c in 0..chunks {
        let base = verts.len() as u32;
        for r in 0..17u32 {
            for q in 0..17u32 {
                verts.push(Vec3::new(
                    q as f32,
                    ((q + r + c as u32) % 5) as f32,
                    r as f32,
                ));
            }
        }
        for r in 0..16u32 {
            for q in 0..16u32 {
                let i = base + r * 17 + q;
                tris.push([i, i + 1, i + 17]);
                tris.push([i + 1, i + 18, i + 17]);
            }
        }
    }
    (verts, tris)
}

fn attach_app() -> App {
    let mut app = App::new();
    app.add_plugins((TaskPoolPlugin::default(), PhysicsPlugins::default()));
    app.finish();
    app.cleanup();
    app
}

#[test]
fn a_finished_task_hands_its_shape_over_and_attaches() {
    let mut app = attach_app();
    let (verts, tris) = grid(4);
    let e = app
        .world_mut()
        .spawn((
            Transform::default(),
            PendingCollider::new(build_collider_task(verts, tris), None),
        ))
        .id();
    for _ in 0..100_000 {
        app.world_mut()
            .run_system_once(finish_colliders)
            .expect("the system runs");
        if app.world().entity(e).contains::<Collider>() {
            break;
        }
        std::thread::yield_now();
    }
    let entity = app.world().entity(e);
    assert!(entity.contains::<Collider>() && entity.contains::<RigidBody>());
    assert!(!entity.contains::<PendingCollider>());
}

#[test]
fn the_attach_budget_spreads_a_burst_without_losing_colliders() {
    const N: usize = 40;
    let mut app = attach_app();
    app.insert_resource(AttachBudget(Duration::ZERO));
    let (verts, tris) = grid(16);
    let entities: Vec<Entity> = (0..N)
        .map(|_| {
            let collider = Collider::trimesh(verts.clone(), tris.clone());
            app.world_mut()
                .spawn((Transform::default(), PendingCollider::ready(collider)))
                .id()
        })
        .collect();
    let attached = |app: &App| {
        entities
            .iter()
            .filter(|e| app.world().entity(**e).contains::<Collider>())
            .count()
    };
    app.world_mut()
        .run_system_once(finish_colliders)
        .expect("the system runs");
    assert_eq!(attached(&app), 1);
    for _ in 0..N {
        app.world_mut()
            .run_system_once(finish_colliders)
            .expect("the system runs");
    }
    assert_eq!(attached(&app), N);
}

/// Skips without `WOW_DATA`.
#[test]
fn a_body_walking_east_into_an_impassable_chunk_is_stopped_at_its_edge() {
    const PIN: [f32; 3] = [-6601.98, -531.87, 335.60];
    const WALL_Y: f32 = 32.0 * TILE_SIZE - 528.0 * CHUNK_SIZE;
    const R: f32 = 0.5;
    const SKIN: f32 = 0.01;
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let chain = mpq::Chain::open(data).expect("open the chain");
    let tiles: Vec<terrain::TileMesh> = [(32, 44), (33, 44)]
        .into_iter()
        .map(|(tx, ty)| terrain::load_tile_mesh(&chain, "Azeroth", tx, ty).expect("the tile"))
        .collect();
    let ground = tiles
        .iter()
        .find_map(|t| terrain::terrain_height_at(&t.chunks, PIN))
        .expect("terrain under the pin");
    let run_east = |walls: bool, z: f32| -> Option<f32> {
        let mut app = physics_app(false);
        for t in &tiles {
            if let Some((v, i)) = terrain_collider_data(&t.chunks) {
                app.world_mut().spawn((
                    RigidBody::Static,
                    Collider::trimesh(v, i),
                    Transform::default(),
                ));
            }
            if let Some((v, i)) = walls.then(|| impassable_wall_data(&t.chunks)).flatten() {
                app.world_mut().spawn((
                    RigidBody::Static,
                    Collider::trimesh(v, i),
                    walk_layers(),
                    Transform::default(),
                ));
            }
        }
        app.update();
        app.world_mut()
            .run_system_once(move |world: WorldCollision<'_, '_>| {
                let from = wow_to_bevy([PIN[0], PIN[1], z]);
                world
                    .cast_body(&Collider::capsule(R, 1.0), from, Vec3::X * 5.0, SKIN)
                    .map(|h| h.distance)
            })
            .expect("the system runs")
    };
    let walking = ground + 1.0 + R;
    assert!(run_east(false, walking).is_none());
    let d = run_east(true, walking).expect("the fence stops the body");
    let leading_edge = -(wow_to_bevy([PIN[0], PIN[1], 0.0]).x + d) - R;
    assert!(
        (leading_edge - (WALL_Y + SKIN)).abs() < 0.05,
        "stopped at y={leading_edge}, the wall is at y={WALL_Y}"
    );
    let floor = tiles
        .iter()
        .flat_map(|t| t.chunks.iter())
        .filter(|c| c.impassable)
        .filter_map(|c| c.positions.iter().map(|p| p[2]).reduce(f32::min))
        .fold(f32::MAX, f32::min);
    assert!(run_east(true, floor - 20.0).is_none());
}
