use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::collision::tests::world_with_mesh;

fn quad(up: bool) -> App {
    let verts = vec![
        Vec3::new(-5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, 5.0),
        Vec3::new(-5.0, 0.0, 5.0),
    ];
    let tris = if up {
        vec![[0u32, 2, 1], [0, 3, 2]]
    } else {
        vec![[0u32, 1, 2], [0, 2, 3]]
    };
    world_with_mesh(verts, tris)
}

fn flat_at(y: f32, half: f32) -> App {
    world_with_mesh(
        vec![
            Vec3::new(-half, y, -half),
            Vec3::new(half, y, -half),
            Vec3::new(half, y, half),
            Vec3::new(-half, y, half),
        ],
        vec![[0u32, 2, 1], [0, 3, 2]],
    )
}

fn capsule() -> Collider {
    Collider::capsule(0.4, 1.0)
}

fn sweep(app: &mut App, from: Vec3, movement: Vec3) -> Option<f32> {
    app.world_mut()
        .run_system_once(move |ms: MoveAndSlide<'_, '_>| {
            cast_move(
                &ms,
                &capsule(),
                from,
                movement,
                0.02,
                &SpatialQueryFilter::default(),
            )
            .map(|h| h.collision_distance)
        })
        .expect("the system runs")
}

/// The capsule's centre is 0.9 above its feet.
#[test]
fn the_band_keeps_a_straddled_slope_and_drops_a_passed_face() {
    let mut seat = world_with_mesh(
        vec![
            Vec3::new(-0.3, 0.5, -0.3),
            Vec3::new(0.3, 0.5, -0.3),
            Vec3::new(0.3, 0.5, 0.3),
            Vec3::new(-0.3, 0.5, 0.3),
        ],
        vec![[0u32, 2, 1], [0, 3, 2]],
    );
    assert_eq!(
        sweep(&mut seat, Vec3::new(0.0, 0.9, 0.0), Vec3::X * 0.2),
        None
    );

    let mut slope = world_with_mesh(
        vec![
            Vec3::new(-5.0, -0.5, -5.0),
            Vec3::new(5.0, 0.5, -5.0),
            Vec3::new(5.0, 0.5, 5.0),
            Vec3::new(-5.0, -0.5, 5.0),
        ],
        vec![[0u32, 2, 1], [0, 3, 2]],
    );
    let feet_under = Vec3::new(0.0, 0.9 - 0.05, 0.0);
    assert_eq!(sweep(&mut slope, feet_under, Vec3::NEG_Y * 0.2), Some(0.0));

    let mut flat = flat_at(0.0, 5.0);
    assert_eq!(sweep(&mut flat, feet_under, Vec3::NEG_Y * 0.2), None);
    let feet_just_under = Vec3::new(0.0, 0.9 - 0.02, 0.0);
    assert_eq!(
        sweep(&mut flat, feet_just_under, Vec3::NEG_Y * 0.2),
        Some(0.0)
    );
}

#[test]
fn a_floor_blocks_a_fall_and_a_backface_does_not() {
    for (up, expect_block) in [(true, true), (false, false)] {
        let hit = quad(up)
            .world_mut()
            .run_system_once(|ms: MoveAndSlide<'_, '_>| {
                cast_move(
                    &ms,
                    &capsule(),
                    Vec3::new(0.0, 3.0, 0.0),
                    Vec3::NEG_Y * 5.0,
                    0.05,
                    &SpatialQueryFilter::default(),
                )
            })
            .expect("the system runs");
        assert_eq!(hit.is_some(), expect_block, "winding up={up}");
        if let Some(h) = hit {
            assert!((h.collision_distance - 2.1).abs() < 1e-3);
            assert!(h.normal1.y > 0.99);
        }
    }
}

#[test]
fn a_floor_is_no_ceiling_from_below() {
    for (up, expect_block) in [(true, false), (false, true)] {
        let hit = quad(up)
            .world_mut()
            .run_system_once(|ms: MoveAndSlide<'_, '_>| {
                cast_move(
                    &ms,
                    &capsule(),
                    Vec3::new(0.0, -3.0, 0.0),
                    Vec3::Y * 5.0,
                    0.05,
                    &SpatialQueryFilter::default(),
                )
            })
            .expect("the system runs");
        assert_eq!(hit.is_some(), expect_block, "winding up={up} from below");
    }
}

#[test]
fn the_slide_falls_through_a_backface_and_rests_on_a_floor() {
    for (up, expect_above) in [(true, true), (false, false)] {
        let end = quad(up)
            .world_mut()
            .run_system_once(|ms: MoveAndSlide<'_, '_>| {
                move_and_slide(
                    &ms,
                    &capsule(),
                    Vec3::new(0.0, 3.0, 0.0),
                    Vec3::NEG_Y * 10.0,
                    Duration::from_secs(1),
                    &MoveAndSlideConfig::default(),
                    &SpatialQueryFilter::default(),
                    |_| MoveAndSlideHitResponse::Accept,
                )
                .position
            })
            .expect("the system runs");
        if expect_above {
            assert!((end.y - 0.9).abs() < 0.1, "rests on the floor, got {end}");
        } else {
            assert!(end.y < -6.0, "falls straight through, got {end}");
        }
    }
}

#[test]
fn a_ray_passes_a_backface_and_stops_on_a_front_face() {
    for (up, expect_hit) in [(true, true), (false, false)] {
        let hit = quad(up)
            .world_mut()
            .run_system_once(|ms: MoveAndSlide<'_, '_>| {
                cast_ray(
                    &ms,
                    Vec3::new(1.0, 2.0, 1.0),
                    Dir3::NEG_Y,
                    10.0,
                    &SpatialQueryFilter::default(),
                )
                .map(|h| h.distance)
            })
            .expect("the system runs");
        assert_eq!(hit.is_some(), expect_hit, "winding up={up}");
        if let Some(d) = hit {
            assert!((d - 2.0).abs() < 1e-4);
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

fn trough(spawn_order: [Side; 2]) -> App {
    let mut app = crate::collision::tests::physics_app(false);
    for side in spawn_order {
        let (x, tris) = match side {
            Side::Left => (-1.0, vec![[0u32, 2, 1], [0, 3, 2]]),
            Side::Right => (1.0, vec![[0u32, 1, 2], [0, 2, 3]]),
        };
        let (a, b) = (Vec3::new(0.0, 0.0, -3.0), Vec3::new(0.0, 0.0, 3.0));
        let outer = Vec3::new(x, 1.0, 0.0);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::trimesh(vec![a, b, outer + b, outer + a], tris),
            Transform::default(),
        ));
    }
    app.update();
    app
}

#[derive(Debug, PartialEq)]
struct Answers {
    sweep: Option<[u32; 7]>,
    slide_normals: Vec<[u32; 3]>,
    slide_end: [u32; 6],
}

fn wedged(app: &mut App) -> Answers {
    app.world_mut()
        .run_system_once(|ms: MoveAndSlide<'_, '_>| {
            let from = Vec3::new(0.0, 0.95, 0.0);
            let bits = |v: Vec3| v.to_array().map(f32::to_bits);
            let sweep = cast_move(
                &ms,
                &capsule(),
                from,
                Vec3::NEG_Y * 0.5,
                0.02,
                &SpatialQueryFilter::default(),
            )
            .map(|h| {
                let [nx, ny, nz] = bits(h.normal1);
                let [px, py, pz] = bits(h.point1);
                [h.collision_distance.to_bits(), nx, ny, nz, px, py, pz]
            });
            let mut slide_normals = Vec::new();
            let out = move_and_slide(
                &ms,
                &capsule(),
                from,
                Vec3::new(0.0, -2.0, 1.0),
                Duration::from_millis(100),
                &MoveAndSlideConfig::default(),
                &SpatialQueryFilter::default(),
                |hit| {
                    slide_normals.push(bits(**hit.normal));
                    *hit.velocity = hit.velocity.reject_from(**hit.normal);
                    MoveAndSlideHitResponse::Accept
                },
            );
            let [x, y, z] = bits(out.position);
            let [vx, vy, vz] = bits(out.projected_velocity);
            Answers {
                sweep,
                slide_normals,
                slide_end: [x, y, z, vx, vy, vz],
            }
        })
        .expect("the system runs")
}

#[test]
fn the_answers_are_the_same_whatever_order_the_faces_arrive_in() {
    let left_first = wedged(&mut trough([Side::Left, Side::Right]));
    let right_first = wedged(&mut trough([Side::Right, Side::Left]));
    assert_eq!(
        left_first.sweep.map(|s| s[0]),
        Some(0.0f32.to_bits()),
        "the capsule starts inside both sides"
    );
    let normal_x = |n: &[u32; 3]| f32::from_bits(n[0]);
    assert!(
        left_first.slide_normals.iter().any(|n| normal_x(n) > 0.0)
            && left_first.slide_normals.iter().any(|n| normal_x(n) < 0.0),
        "the slide meets both sides"
    );
    assert_eq!(left_first, right_first);
}
