use std::sync::Arc;
use std::time::Duration;

use bevy::animation::graph::AnimationNodeIndex;
use bevy::mesh::MeshTag;

use super::lazy::{LazyRig, SkinnedTwin, reap_parked_rigs};
use super::*;
use crate::rig::{ClipEvent, GlobalBone, GlobalSeqChannel, ModelJoint, RigPalettes};
use crate::visibility::alpha_bits;

fn clip(anim_id: u16, seq_index: usize, node: usize) -> AnimClip {
    AnimClip {
        anim_id,
        seq_index,
        node: AnimationNodeIndex::new(node),
        looping: true,
        duration: 2.0,
        move_speed: 0.0,
        blend_time: 0.0,
        bounds_min: Vec3::ZERO,
        bounds_max: Vec3::ZERO,
        frequency: 0,
        replay: (0, 0),
        poses_bones: true,
        events: Arc::from([]),
    }
}

fn anims(clips: Vec<AnimClip>, moving_idle: Option<usize>, gseq: bool) -> ModelAnimations {
    ModelAnimations {
        graph: Handle::default(),
        clips,
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: if gseq {
            vec![GlobalBone {
                bone: 1,
                translation: None,
                rotation: None,
                scale: Some(GlobalSeqChannel {
                    period: 1.167,
                    keys: vec![(0.0, Vec3::ONE), (0.5, Vec3::splat(1.2))],
                }),
            }]
        } else {
            Vec::new()
        },
        moving_idle,
        pose: Arc::default(),
    }
}

fn skeleton(joints: usize) -> ModelSkeleton {
    ModelSkeleton {
        joints: (0..joints)
            .map(|_| ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            })
            .collect(),
        spine_bone: None,
        head_bone: None,
    }
}

#[test]
fn a_model_is_rigged_only_when_something_moves_off_its_rest_pose() {
    assert!(matches!(
        classify(&skeleton(3), None),
        DoodadAnimTier::Static
    ));
    let a = anims(vec![clip(0, 0, 1)], Some(0), true);
    assert!(matches!(
        classify(&skeleton(0), Some(&a)),
        DoodadAnimTier::Static
    ));
    let a = anims(vec![clip(0, 0, 1)], None, false);
    assert!(matches!(
        classify(&skeleton(3), Some(&a)),
        DoodadAnimTier::Static
    ));
    let a = anims(vec![clip(0, 0, 1)], None, true);
    assert!(matches!(
        classify(&skeleton(3), Some(&a)),
        DoodadAnimTier::GlobalSeqOnly
    ));
    let a = anims(vec![clip(0, 0, 1)], Some(0), true);
    assert!(matches!(
        classify(&skeleton(3), Some(&a)),
        DoodadAnimTier::MovingIdle(c) if c.anim_id == 0
    ));
}

#[test]
fn a_still_model_whose_idle_sounds_runs_its_idle_clock_alone() {
    let mut idle = clip(0, 0, 1);
    idle.events = Arc::from([ClipEvent {
        time: 0.0,
        ident: *b"$DSL",
        data: 7,
        bone: 0,
        offset: Vec3::ZERO,
        point: Vec3::ZERO,
    }]);
    let sounding = anims(vec![idle.clone()], None, false);
    assert!(matches!(
        arm(&skeleton(0), &sounding),
        Some(Arm::ClockOnly(c)) if c.anim_id == 0
    ));
    let sounding_gseq = anims(vec![idle], None, true);
    assert!(matches!(
        arm(&skeleton(3), &sounding_gseq),
        Some(Arm::ClockOnly(_))
    ));
    let silent = anims(vec![clip(0, 0, 1)], None, false);
    assert!(arm(&skeleton(3), &silent).is_none());
    let gseq = anims(vec![clip(0, 0, 1)], None, true);
    assert!(matches!(
        arm(&skeleton(3), &gseq),
        Some(Arm::GlobalSeqsOnly)
    ));
    let moving = anims(vec![clip(0, 0, 1)], Some(0), false);
    assert!(matches!(arm(&skeleton(3), &moving), Some(Arm::Posed(_))));
}

fn host(batches: Vec<Entity>, rerolls_at: f32, anim_id: Option<u16>, gate: Gate) -> DoodadAnimHost {
    DoodadAnimHost {
        seen_by: if batches.is_empty() {
            SeenBy::Bounds(DrawSetGate::sphere(1.0, Vec3::ZERO))
        } else {
            SeenBy::Batches(batches)
        },
        clip: None,
        armed_at: 0.0,
        rerolls_at,
        anim_id,
        gate,
        parked_at: 0.0,
        own_stream: None,
    }
}

const COMMON_WEIGHT: u16 = 31129;
const RARE_WEIGHT: u16 = 1638;
const ROLLS: u32 = 400;

fn lightning() -> ModelAnimations {
    let mut a = anims(vec![clip(0, 0, 1), clip(0, 1, 2)], Some(0), false);
    a.clips[0].frequency = COMMON_WEIGHT;
    a.clips[0].duration = 1.333;
    a.clips[1].frequency = RARE_WEIGHT;
    a.clips[1].duration = 1.300;
    a
}

fn reroll_app() -> App {
    let mut app = App::new();
    app.init_resource::<Time>();
    app.init_resource::<AnimRng>();
    app.add_systems(Update, reroll_doodad_variation);
    app
}

fn step(app: &mut App, ms: u64) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_millis(ms));
    app.update();
}

#[test]
fn a_placed_doodad_rolls_a_fresh_variation_every_window() {
    let mut app = reroll_app();
    let h = app
        .world_mut()
        .spawn((
            host(Vec::new(), f32::NEG_INFINITY, Some(0), Gate::Drawn),
            lightning(),
            AnimationPlayer::default(),
            Transform::default(),
        ))
        .id();
    app.update();
    let mut seen = std::collections::HashMap::new();
    for _ in 0..ROLLS {
        step(&mut app, 1400);
        let host = app
            .world()
            .entity(h)
            .get::<DoodadAnimHost>()
            .expect("a host");
        let clip = host.clip.expect("armed");
        assert!((host.rerolls_at - host.armed_at - clip.duration).abs() < 1e-3);
        *seen.entry(clip.node).or_insert(0u32) += 1;
    }
    let rare = seen.get(&AnimationNodeIndex::new(2)).copied().unwrap_or(0);
    let common = seen.get(&AnimationNodeIndex::new(1)).copied().unwrap_or(0);
    assert_eq!(rare + common, ROLLS);
    let p = f64::from(RARE_WEIGHT) / (f64::from(COMMON_WEIGHT) + f64::from(RARE_WEIGHT));
    let (mean, sd) = (
        f64::from(ROLLS) * p,
        (f64::from(ROLLS) * p * (1.0 - p)).sqrt(),
    );
    assert!(
        (f64::from(rare) - mean).abs() <= 3.0 * sd,
        "the rare variation came up {rare} times, {mean:.0} expected"
    );
}

fn variations_played(at: Vec3, session_draws_first: usize) -> (Vec<AnimationNodeIndex>, AnimRng) {
    let mut app = reroll_app();
    for _ in 0..session_draws_first {
        app.world_mut().resource_mut::<AnimRng>().draw();
    }
    let h = app
        .world_mut()
        .spawn((
            host(Vec::new(), f32::NEG_INFINITY, Some(0), Gate::Drawn),
            lightning(),
            AnimationPlayer::default(),
            Transform::from_translation(at),
        ))
        .id();
    let played = (0..ROLLS)
        .map(|_| {
            step(&mut app, 1400);
            let host = app.world().entity(h).get::<DoodadAnimHost>();
            host.and_then(|h| h.clip).expect("armed").node
        })
        .collect();
    (played, *app.world().resource::<AnimRng>())
}

#[test]
fn a_doodad_rolls_on_a_stream_of_its_own() {
    let at = Vec3::new(-9433.0, 44.0, 57.0);
    let (played, mut session) = variations_played(at, 0);
    let (after_others, _) = variations_played(at, 7);
    assert!(played == after_others, "rolls made elsewhere moved its own");
    assert_eq!(
        session.draw(),
        AnimRng::default().draw(),
        "its rolls moved the session's"
    );
    let (a_yard_off, _) = variations_played(at + Vec3::X, 0);
    assert!(played != a_yard_off, "a doodad a yard off rolled the same");
}

#[test]
fn a_global_sequence_only_host_is_never_armed() {
    let mut app = reroll_app();
    let h = app
        .world_mut()
        .spawn((
            host(Vec::new(), f32::NEG_INFINITY, None, Gate::Drawn),
            anims(Vec::new(), None, true),
            Transform::default(),
        ))
        .id();
    for _ in 0..8 {
        step(&mut app, 1400);
    }
    let host = app
        .world()
        .entity(h)
        .get::<DoodadAnimHost>()
        .expect("a host");
    assert!(host.clip.is_none());
    assert_eq!(host.rerolls_at, f32::NEG_INFINITY);
}

#[test]
fn an_undrawn_host_keeps_rolling_but_leaves_its_player_stopped() {
    let mut app = reroll_app();
    let h = app
        .world_mut()
        .spawn((
            host(Vec::new(), f32::NEG_INFINITY, Some(0), Gate::Parked),
            lightning(),
            AnimationPlayer::default(),
            Transform::default(),
        ))
        .id();
    app.update();
    let first = app
        .world()
        .entity(h)
        .get::<DoodadAnimHost>()
        .expect("a host")
        .rerolls_at;
    for _ in 0..3 {
        step(&mut app, 1400);
    }
    let e = app.world().entity(h);
    assert!(e.get::<DoodadAnimHost>().expect("a host").rerolls_at > first);
    let player = e.get::<AnimationPlayer>().expect("a player");
    assert_eq!(player.playing_animations().count(), 0);
}

fn gate_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.build().disable::<bevy::time::TimePlugin>(),
        AssetPlugin::default(),
    ));
    app.init_resource::<Time>();
    app.init_resource::<RigPalettes>();
    app.init_resource::<crate::portal::ExteriorWindows>();
    app.init_resource::<crate::portal::CameraInteriorClaim>();
    app.init_asset::<Mesh>();
    app.add_systems(Update, (gate_doodad_anim, reap_parked_rigs).chain());
    app
}

#[test]
fn a_hidden_doodad_stops_and_resumes_where_the_clock_says() {
    for (resumed, resume_ms) in [("running", 700), ("held", 0)] {
        let mut app = gate_app();
        let mesh = app.world_mut().spawn(Visibility::Inherited).id();
        let node = AnimationNodeIndex::new(1);
        let mut player = AnimationPlayer::default();
        player.play(node).repeat();
        let mut h = host(vec![mesh], f32::INFINITY, Some(0), Gate::Drawn);
        h.clip = Some(ArmedClip {
            node,
            duration: 2.0,
        });
        let h = app.world_mut().spawn((h, player)).id();
        let playing = |app: &App| {
            app.world()
                .entity(h)
                .get::<AnimationPlayer>()
                .expect("a player")
                .playing_animations()
                .count()
        };
        let show = |app: &mut App, v: Visibility| {
            *app.world_mut()
                .entity_mut(mesh)
                .get_mut::<Visibility>()
                .expect("visibility") = v;
        };
        step(&mut app, 300);
        assert_eq!(playing(&app), 1);
        show(&mut app, Visibility::Hidden);
        step(&mut app, 300);
        assert_eq!(playing(&app), 0);
        assert!(app.world().entity(h).contains::<crate::rig::AnimParked>());
        show(&mut app, Visibility::Inherited);
        step(&mut app, resume_ms);
        assert_eq!(playing(&app), 1);
        let seek = app
            .world()
            .entity(h)
            .get::<AnimationPlayer>()
            .and_then(|p| {
                p.animation(node)
                    .map(bevy::animation::ActiveAnimation::seek_time)
            })
            .expect("resumed");
        let time = app.world().resource::<Time>();
        let advanced = (seek + time.delta_secs()).rem_euclid(2.0);
        let clock = time.elapsed_secs().rem_euclid(2.0);
        assert!(
            (advanced - clock).abs() < 1e-4,
            "resumed with the clock {resumed}: {advanced} against {clock}"
        );
    }
}

fn lazy_host(app: &mut App, visible: bool) -> (Entity, Entity) {
    let unskinned = Handle::<Mesh>::default();
    let skinned = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .reserve_handle();
    let part = app
        .world_mut()
        .spawn((
            Mesh3d(unskinned.clone()),
            MeshTag(alpha_bits(1.0)),
            SkinnedTwin { skinned, unskinned },
            if visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ))
        .id();
    let h = app
        .world_mut()
        .spawn((
            host(vec![part], f32::INFINITY, Some(0), Gate::New),
            LazyRig {
                ibp: Arc::from([Mat4::IDENTITY].as_slice()),
                parts: vec![part],
            },
        ))
        .id();
    let pose = RigPose::new(h, &skeleton(1));
    app.world_mut().entity_mut(h).insert(pose);
    (h, part)
}

fn rig_slot(app: &App, h: Entity) -> Option<u16> {
    app.world().entity(h).get::<RigSkin>().map(|r| r.slot)
}

#[test]
fn a_doodad_is_armed_the_frame_it_spawns_and_parked_the_next_if_unseen() {
    let mut app = gate_app();
    app.world_mut().spawn((
        crate::view::WorldCamera,
        GlobalTransform::default(),
        Frustum::default(),
        Projection::default(),
    ));
    let mesh = app.world_mut().spawn(Visibility::Hidden).id();
    let node = AnimationNodeIndex::new(1);
    let mut player = AnimationPlayer::default();
    player.play(node).repeat();
    let mut h = host(vec![mesh], f32::INFINITY, Some(0), Gate::New);
    h.clip = Some(ArmedClip {
        node,
        duration: 2.0,
    });
    let h = app.world_mut().spawn((h, player)).id();
    let state = |app: &App| {
        let e = app.world().entity(h);
        (
            e.get::<DoodadAnimHost>().expect("a host").gate,
            e.contains::<crate::rig::AnimParked>(),
        )
    };
    app.update();
    assert_eq!(state(&app), (Gate::ArmedAtSpawn, false));
    app.update();
    assert_eq!(state(&app), (Gate::Parked, true));
}

#[test]
fn a_rig_takes_its_slot_on_its_second_drawn_frame() {
    let mut app = gate_app();
    let (h, part) = lazy_host(&mut app, false);
    app.update();
    app.update();
    assert_eq!(rig_slot(&app, h), None, "hidden");
    *app.world_mut()
        .entity_mut(part)
        .get_mut::<Visibility>()
        .expect("visibility") = Visibility::Inherited;
    app.update();
    assert_eq!(rig_slot(&app, h), None, "one drawn frame is not enough");
    app.update();
    let slot = rig_slot(&app, h).expect("a slot on the second");
    let e = app.world().entity(part);
    let twin = e.get::<SkinnedTwin>().expect("a twin").skinned.clone();
    assert_eq!(e.get::<Mesh3d>().expect("a mesh").0, twin);
    let tag = e.get::<MeshTag>().expect("a tag").0;
    assert_eq!(((tag >> 19) & 0x7ff) as u16, slot);
}

#[test]
fn a_rig_refused_by_a_full_palette_takes_the_first_freed_slot() {
    let mut app = gate_app();
    let hoard: Vec<RigSkin> = {
        let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
        let ibp: Arc<[Mat4]> = Arc::from([Mat4::IDENTITY].as_slice());
        std::iter::from_fn(|| RigSkin::allocate(&mut palettes, ibp.clone())).collect()
    };
    assert_eq!(app.world().resource::<RigPalettes>().free_slots(), 0);
    let (h, _) = lazy_host(&mut app, true);
    app.update();
    app.update();
    assert_eq!(rig_slot(&app, h), None);
    let freed = hoard[0].slot;
    app.world_mut().resource_mut::<RigPalettes>().free(freed);
    app.update();
    assert!(rig_slot(&app, h).is_some());
}

#[test]
fn a_long_parked_rig_gives_its_slot_back_only_under_pressure() {
    let mut app = gate_app();
    let (h, part) = lazy_host(&mut app, true);
    app.update();
    app.update();
    assert!(rig_slot(&app, h).is_some());
    *app.world_mut()
        .entity_mut(part)
        .get_mut::<Visibility>()
        .expect("visibility") = Visibility::Hidden;
    app.update();
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs(3));
    app.update();
    assert!(rig_slot(&app, h).is_some(), "room enough: kept");
    let _hoard: Vec<RigSkin> = {
        let mut palettes = app.world_mut().resource_mut::<RigPalettes>();
        let ibp: Arc<[Mat4]> = Arc::from([Mat4::IDENTITY].as_slice());
        (0..crate::rig::MAX_RIG_SLOTS - 1 - 256)
            .filter_map(|_| RigSkin::allocate(&mut palettes, ibp.clone()))
            .collect()
    };
    app.update();
    app.update();
    assert_eq!(rig_slot(&app, h), None, "reaped");
    let e = app.world().entity(part);
    assert_eq!(
        e.get::<Mesh3d>().expect("a mesh").0,
        e.get::<SkinnedTwin>().expect("a twin").unskinned
    );
}
