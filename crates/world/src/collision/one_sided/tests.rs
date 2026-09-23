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
