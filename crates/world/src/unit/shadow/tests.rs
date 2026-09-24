use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;
use std::time::Duration;

use avian3d::prelude::Collider;
use bevy::animation::graph::AnimationNodeIndex;

use super::*;
use crate::collision::GroundDecalSurface;
use crate::rig::AnimClip;

#[test]
fn the_ramp_rises_over_the_bottom_sixth_and_falls_over_the_top_sixth() {
    assert_eq!(height_ramp(0.0), 0.0);
    assert!((height_ramp(1.0 / 12.0) - 0.5).abs() < 1e-6);
    assert!((height_ramp(1.0 / 6.0) - 1.0).abs() < 1e-6);
    assert_eq!(height_ramp(0.5), 1.0);
    assert!((height_ramp(5.0 / 6.0) - 1.0).abs() < 1e-6);
    assert!((height_ramp(11.0 / 12.0) - 0.5).abs() < 1e-6);
    assert_eq!(height_ramp(1.0), 0.0);
    assert_eq!(height_ramp(-1.0), 0.0);
    assert_eq!(height_ramp(2.0), 0.0);
}

#[test]
fn the_box_is_clamped_before_the_scale_then_turned_and_bounded() {
    let stand = (Vec3::new(-7.0, 0.0, -0.25), Vec3::new(1.0, 2.0, 0.25));
    let f = shadow_frame(Vec3::ZERO, Quat::IDENTITY, 2.0, stand).expect("a box");
    assert_eq!(
        (f.min_x, f.max_x, f.min_z, f.max_z),
        (-10.0, 2.0, -0.5, 0.5)
    );
    assert_eq!((f.min_y, f.max_y), (-2.0 * DOWN_PER_UP, 2.0));
    let f = shadow_frame(Vec3::ZERO, Quat::from_rotation_y(FRAC_PI_2), 1.0, stand).expect("a box");
    let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
    assert!(near(f.min_x, -0.25) && near(f.max_x, 0.25));
    assert!(near(f.min_z, -1.0) && near(f.max_z, 5.0));
    let flat = (Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 2.0, 0.0));
    assert!(shadow_frame(Vec3::ZERO, Quat::IDENTITY, 1.0, flat).is_none());
}

fn clip(anim_id: u16, bounds_min: Vec3, bounds_max: Vec3) -> AnimClip {
    AnimClip {
        anim_id,
        seq_index: 0,
        node: AnimationNodeIndex::new(0),
        looping: true,
        duration: 1.0,
        move_speed: 0.0,
        blend_time: 0.0,
        bounds_min,
        bounds_max,
        frequency: 0,
        replay: (0, 0),
        poses_bones: true,
        events: Arc::from([]),
    }
}

fn anims(clips: Vec<AnimClip>) -> ModelAnimations {
    ModelAnimations {
        graph: Handle::default(),
        clips,
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        moving_idle: None,
        pose: Arc::default(),
    }
}

const STAND_MIN: Vec3 = Vec3::new(-0.5, 0.0, -0.25);
const STAND_MAX: Vec3 = Vec3::new(0.5, 2.0, 0.25);

fn world_with_ground() -> App {
    let mut app = App::new();
    app.init_resource::<Time>()
        .add_systems(Update, update_shadows);
    let verts = vec![
        Vec3::new(-10.0, 0.0, -10.0),
        Vec3::new(10.0, 0.0, -10.0),
        Vec3::new(10.0, 0.0, 10.0),
        Vec3::new(-10.0, 0.0, 10.0),
    ];
    app.world_mut().spawn((
        Collider::trimesh(verts, vec![[0, 2, 1], [0, 3, 2]]),
        GroundDecalSurface,
    ));
    app
}

fn unit(app: &mut App, at: Vec3, clips: Vec<AnimClip>) -> Entity {
    app.world_mut()
        .spawn((
            Transform::from_translation(at),
            anims(clips),
            UnitBody {
                display: 0,
                model: String::new(),
                skins: [None, None, None],
                character: None,
            },
        ))
        .id()
}

fn shadow(app: &App, unit: Entity) -> &BlobShadow {
    app.world().get::<BlobShadow>(unit).expect("a unit casts")
}

fn step(app: &mut App, secs: f32) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(secs));
    app.update();
}

#[test]
fn a_standing_unit_lays_its_box_on_the_ground_at_its_feet() {
    let mut app = world_with_ground();
    let feet = Vec3::new(1.0, 0.0, 2.0);
    let u = unit(&mut app, feet, vec![clip(0, STAND_MIN, STAND_MAX)]);
    step(&mut app, 0.0);
    let s = shadow(&app, u);
    assert!(!s.verts.is_empty());
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &s.verts {
        lo = lo.min(Vec3::from(v.pos));
        hi = hi.max(Vec3::from(v.pos));
        assert!(
            (v.color[3] - 1.0).abs() < 1e-6,
            "the feet sit in the ramp's plateau"
        );
    }
    assert!((lo - (feet + STAND_MIN.with_y(0.0))).abs().max_element() < 1e-5);
    assert!((hi - (feet + STAND_MAX.with_y(0.0))).abs().max_element() < 1e-5);
}

#[test]
fn the_shadow_is_as_faint_as_the_unit_and_gone_with_it() {
    let mut app = world_with_ground();
    let u = unit(&mut app, Vec3::ZERO, vec![clip(0, STAND_MIN, STAND_MAX)]);
    app.world_mut().entity_mut(u).insert(UnitAppear::at(0.0));
    step(&mut app, 1.0);
    let alpha = shadow(&app, u).verts[0].color[3];
    assert!(
        (alpha - 0.125).abs() < 1e-6,
        "half the ramp is an eighth: {alpha}"
    );
    let mut owner = UnitAlpha::default();
    owner.alpha = 0.0;
    app.world_mut()
        .entity_mut(u)
        .remove::<UnitAppear>()
        .insert(owner);
    step(&mut app, 0.1);
    assert!(shadow(&app, u).verts.is_empty());
    app.world_mut().entity_mut(u).insert(UnitAlpha::default());
    step(&mut app, 0.1);
    assert!(!shadow(&app, u).verts.is_empty(), "and back when it is");
}

#[test]
fn nothing_is_cast_off_the_ground_or_without_a_stand() {
    let mut app = world_with_ground();
    let high = unit(
        &mut app,
        Vec3::Y * 10.0,
        vec![clip(0, STAND_MIN, STAND_MAX)],
    );
    let no_stand = unit(&mut app, Vec3::ZERO, vec![clip(4, STAND_MIN, STAND_MAX)]);
    step(&mut app, 0.0);
    assert!(shadow(&app, high).verts.is_empty());
    assert!(shadow(&app, no_stand).verts.is_empty());
}

#[test]
fn a_shadow_is_rebuilt_only_when_its_unit_moves() {
    let mut app = world_with_ground();
    let u = unit(&mut app, Vec3::ZERO, vec![clip(0, STAND_MIN, STAND_MAX)]);
    step(&mut app, 0.0);
    let mark = |app: &mut App| {
        app.world_mut()
            .get_mut::<BlobShadow>(u)
            .expect("casts")
            .verts[0]
            .color[0] = 0.5;
    };
    mark(&mut app);
    step(&mut app, 0.1);
    assert_eq!(shadow(&app, u).verts[0].color[0], 0.5, "kept");
    app.world_mut()
        .get_mut::<Transform>(u)
        .expect("placed")
        .translation
        .x = 0.5;
    step(&mut app, 0.1);
    assert_eq!(shadow(&app, u).verts[0].color[0], 1.0, "rebuilt");
}
