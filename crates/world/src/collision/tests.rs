use avian3d::character_controller::move_and_slide::MoveAndSlide;
use bevy::ecs::system::RunSystemOnce;
use bevy::transform::TransformPlugin;

use super::*;

/// A headless app whose physics schedule runs every update.
pub(crate) fn physics_app(stripped: bool) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin));
    if stripped {
        app.add_plugins(physics_plugins(PostUpdate));
    } else {
        app.add_plugins(PhysicsPlugins::new(PostUpdate));
    }
    app.finish();
    app.cleanup();
    app
}

/// A world holding one static trimesh on the default layer, trees built.
pub(crate) fn world_with_mesh(verts: Vec<Vec3>, tris: Vec<[u32; 3]>) -> App {
    let mut app = physics_app(false);
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::trimesh(verts, tris),
        Transform::default(),
    ));
    app.update();
    app
}

fn waterline_world(cell: f32, cells: usize) -> App {
    let mut app = physics_app(false);
    let half = cell * cells as f32 * 0.5;
    let mut verts = Vec::with_capacity((cells + 1) * (cells + 1));
    for r in 0..=cells {
        for c in 0..=cells {
            verts.push(Vec3::new(
                c as f32 * cell - half,
                0.0,
                r as f32 * cell - half,
            ));
        }
    }
    let stride = (cells + 1) as u32;
    let mut tris = Vec::with_capacity(cells * cells * 2);
    for r in 0..cells {
        for c in 0..cells {
            let i = (r * (cells + 1) + c) as u32;
            tris.push([i, i + stride, i + 1]);
            tris.push([i + 1, i + stride, i + stride + 1]);
        }
    }
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::trimesh(verts, tris),
        liquid_layers(),
        Transform::default(),
    ));
    app.update();
    app
}

fn descend_camera(app: &mut App, liquid: bool) -> Option<f32> {
    app.world_mut()
        .run_system_once(move |c: WorldCollision<'_, '_>| {
            c.cast_camera(Vec3::new(0.0, 3.0, 0.0), Vec3::new(0.0, -6.0, 0.0), liquid)
        })
        .expect("the system runs")
}

fn descend_body(app: &mut App) -> Option<f32> {
    app.world_mut()
        .run_system_once(|c: WorldCollision<'_, '_>| {
            c.cast_body(
                &Collider::sphere(CAMERA_PROBE_RADIUS),
                Vec3::new(0.0, 3.0, 0.0),
                Vec3::new(0.0, -6.0, 0.0),
                0.0,
            )
            .map(|h| h.distance)
        })
        .expect("the system runs")
}

#[test]
fn the_waterline_stops_the_camera_only_when_asked() {
    let mut app = waterline_world(10.0, 1);
    let on = descend_camera(&mut app, true);
    assert!(on.is_some_and(|d| (d - 3.0).abs() < 0.01), "{on:?}");
    assert_eq!(descend_camera(&mut app, false), None);
    assert_eq!(descend_body(&mut app), None);
}

/// A surface-swimming human male: feet `0.75·h` under the plane, the boom from his head, 15 yd.
const SURFACE_DEPTH: f32 = 0.75 * 2.031;
const HEAD_OVER_FEET: f32 = 2.027_777_7 - 1.0 / 3.0;
const ZOOM: f32 = 15.0;
const UNCORRECTED_PIVOT: f32 = 1.512_012;

fn boom_from(feet_y: f32, (pivot_over, head_over): (f32, f32), pitch: f32) -> (Vec3, Vec3, f32) {
    let feet = Vec3::new(0.0, feet_y, 0.0);
    let head = feet + Vec3::Y * head_over;
    let pivot = feet + Vec3::Y * pivot_over;
    let fwd = Quat::from_euler(EulerRot::YXZ, 0.0, pitch, 0.0) * Vec3::NEG_Z;
    let boom = (pivot - fwd * ZOOM) - head;
    (head, boom, boom.length())
}

fn open_arm(app: &mut App, feet_y: f32, geo: (f32, f32), pitch: f32) -> f32 {
    let (head, boom, len) = boom_from(feet_y, geo, pitch);
    app.world_mut()
        .run_system_once(move |c: WorldCollision<'_, '_>| {
            c.cast_camera(head, boom, true).unwrap_or(len)
        })
        .expect("the system runs")
}

/// The query as one sphere sweep carrying the liquid layer: the shape in which the camera slams
/// onto a surface swimmer.
fn open_arm_swept(app: &mut App, feet_y: f32, geo: (f32, f32), pitch: f32) -> f32 {
    let (head, boom, len) = boom_from(feet_y, geo, pitch);
    app.world_mut()
        .run_system_once(move |ms: MoveAndSlide<'_, '_>| {
            ms.cast_move(
                &Collider::sphere(CAMERA_PROBE_RADIUS),
                head,
                Quat::IDENTITY,
                boom,
                0.0,
                &SpatialQueryFilter::from_mask(LayerMask(
                    CollisionLayer::Default.to_bits()
                        | CollisionLayer::Camera.to_bits()
                        | CollisionLayer::Liquid.to_bits(),
                )),
            )
            .map_or(len, |h| h.distance)
        })
        .expect("the system runs")
}

fn uncorrected(_feet_y: f32) -> (f32, f32) {
    (UNCORRECTED_PIVOT, HEAD_OVER_FEET)
}

/// The corridor pins the pivot and lifts the sweep origin to `surface + 2/9`.
fn corrected(feet_y: f32) -> (f32, f32) {
    let floor = -feet_y + 2.0 / 9.0;
    (floor, HEAD_OVER_FEET.max(floor))
}

fn worst_over_depth(
    app: &mut App,
    arm: impl Fn(&mut App, f32, (f32, f32), f32) -> f32,
    geo: impl Fn(f32) -> (f32, f32),
    pitch: f32,
) -> f32 {
    const SPAN: f32 = 0.30;
    const STEPS: usize = 600;
    let mut worst: f32 = 0.0;
    let mut prev: Option<f32> = None;
    for i in 0..=STEPS {
        let feet_y = -SURFACE_DEPTH - SPAN * 0.5 + SPAN * i as f32 / STEPS as f32;
        let d = arm(app, feet_y, geo(feet_y), pitch);
        if let Some(q) = prev {
            worst = worst.max((d - q).abs());
        }
        prev = Some(d);
    }
    worst
}

/// Walking a surface swimmer's depth through fractions of a millimetre swings the uncorrected
/// camera by yards; with the corridor, depth is no longer an input.
#[test]
fn a_swimmers_settle_cannot_move_the_camera_once_the_corridor_holds_it() {
    const CELL: f32 = 33.333_332 / 8.0;
    let mut app = waterline_world(CELL, 32);
    let control = worst_over_depth(
        &mut app,
        open_arm_swept,
        uncorrected,
        (-2.0f32).to_radians(),
    );
    assert!(control > 5.0, "the control must swing, got {control}");
    for pitch_deg in [-25.0f32, -10.0, -2.0, -0.5, 0.0, 0.5, 2.0, 10.0, 25.0] {
        let step = worst_over_depth(&mut app, open_arm, corrected, pitch_deg.to_radians());
        assert!(step < 0.1, "at {pitch_deg} deg the arm moved {step} yd");
    }
}

/// The probe sphere is fatter than the corridor's clearance, so the waterline leg is a ray: a
/// level boom behind a surface swimmer stays open, and one aimed under the water still stops on it.
#[test]
fn a_level_boom_behind_a_surface_swimmer_is_not_pinned_to_the_water() {
    const CELL: f32 = 33.333_332 / 8.0;
    let mut app = waterline_world(CELL, 32);
    let feet_y = -SURFACE_DEPTH;
    let geo = corrected(feet_y);
    let control = open_arm_swept(&mut app, feet_y, geo, 0.0);
    assert!(control < 0.01, "the control must pin, got {control}");
    for deg in [-25.0f32, -10.0, -2.0, -0.5, 0.0, 0.25, 0.5] {
        let open = open_arm(&mut app, feet_y, geo, deg.to_radians());
        assert!(open > ZOOM - 0.01, "at {deg} deg the arm clipped at {open}");
    }
    let aimed = open_arm(&mut app, feet_y, geo, 20.0f32.to_radians());
    assert!((aimed - 0.65).abs() < 0.05, "{aimed}");
}

#[test]
fn dropping_the_broad_phase_removes_the_pairs_but_not_the_casts() {
    let slab = |app: &mut App| {
        for (half, body) in [(5.0, RigidBody::Static), (2.0, RigidBody::Kinematic)] {
            app.world_mut().spawn((
                body,
                Collider::trimesh(
                    vec![
                        Vec3::new(-half, 0.0, -half),
                        Vec3::new(half, 0.0, -half),
                        Vec3::new(half, 0.0, half),
                        Vec3::new(-half, 0.0, half),
                    ],
                    vec![[0u32, 2, 1], [0, 3, 2]],
                ),
                Transform::default(),
            ));
        }
        app.update();
        app.update();
    };
    let mut stock = physics_app(false);
    slab(&mut stock);
    assert!(
        !stock
            .world()
            .resource::<ContactGraph>()
            .active_pairs()
            .is_empty()
    );

    let mut stripped = physics_app(true);
    slab(&mut stripped);
    assert!(
        stripped
            .world()
            .resource::<ContactGraph>()
            .active_pairs()
            .is_empty()
    );
    let hit = stripped
        .world_mut()
        .run_system_once(|c: WorldCollision<'_, '_>| {
            c.ray_body(Vec3::new(0.0, 5.0, 0.0), Dir3::NEG_Y, 10.0)
        })
        .expect("the system runs");
    assert!(hit.is_some_and(|h| (h.distance - 5.0).abs() < 1e-4));
}
