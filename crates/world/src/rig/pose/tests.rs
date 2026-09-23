//! The evaluator against Bevy's own: the same graph played on two rigs, one whose bones are Bevy
//! animation targets that `animate_targets` writes, one a [`RigPose`] the evaluator writes, and
//! every bone compared bit for bit.

use std::sync::Arc;
use std::time::Duration;

use bevy::animation::animated_field;
use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{AnimatedBy, AnimationClip, AnimationPlugin, AnimationTargetId};
use bevy::asset::AssetPlugin;
use bevy::time::TimeUpdateStrategy;

use super::super::bake::ModelJoint;
use super::super::source::{PoseBone, PoseClip, PoseTrack};
use super::*;

#[derive(Clone, Default)]
struct ClipSpec {
    bones: Vec<(u16, BoneSpec)>,
    mask: u64,
}

#[derive(Clone, Default)]
struct BoneSpec {
    t: Vec<(f32, Vec3)>,
    r: Vec<(f32, Quat)>,
    s: Vec<(f32, Vec3)>,
}

fn target(bone: u16) -> AnimationTargetId {
    AnimationTargetId::from_name(&Name::new(format!("bone {bone}")))
}

fn build(
    app: &mut App,
    specs: &[ClipSpec],
    bone_masks: Vec<u64>,
) -> (Handle<AnimationGraph>, Vec<AnimationNodeIndex>, PoseSource) {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let mut pose = PoseSource {
        bone_masks,
        ..PoseSource::default()
    };
    for (i, &bits) in pose.bone_masks.iter().enumerate() {
        for group in 0..8 {
            if bits & (1 << group) != 0 {
                graph.add_target_to_mask_group(target(i as u16), group);
            }
        }
    }
    let mut nodes = Vec::new();
    for spec in specs {
        let mut clip = AnimationClip::default();
        let mut pose_clip = PoseClip::default();
        for (bone, keys) in &spec.bones {
            let t = target(*bone);
            if keys.t.len() >= 2 {
                clip.add_curve_to_target(
                    t,
                    AnimatableCurve::new(
                        animated_field!(Transform::translation),
                        AnimatableKeyframeCurve::new(keys.t.iter().copied()).expect("keys"),
                    ),
                );
            }
            if keys.r.len() >= 2 {
                clip.add_curve_to_target(
                    t,
                    AnimatableCurve::new(
                        animated_field!(Transform::rotation),
                        AnimatableKeyframeCurve::new(keys.r.iter().copied()).expect("keys"),
                    ),
                );
            }
            if keys.s.len() >= 2 {
                clip.add_curve_to_target(
                    t,
                    AnimatableCurve::new(
                        animated_field!(Transform::scale),
                        AnimatableKeyframeCurve::new(keys.s.iter().copied()).expect("keys"),
                    ),
                );
            }
            let track = |k: &[(f32, _)]| {
                if k.len() >= 2 {
                    PoseTrack::new(k)
                } else {
                    PoseTrack::default()
                }
            };
            pose_clip.push(PoseBone {
                bone: *bone,
                translation: track(&keys.t),
                rotation: if keys.r.len() >= 2 {
                    PoseTrack::new(&keys.r)
                } else {
                    PoseTrack::default()
                },
                scale: track(&keys.s),
            });
        }
        let handle = app
            .world_mut()
            .resource_mut::<Assets<AnimationClip>>()
            .add(clip);
        let clip_index = u32::try_from(pose.clips.len()).expect("few clips");
        pose.clips.push(pose_clip);
        let node = if spec.mask == 0 {
            graph.add_clip(handle, 1.0, root)
        } else {
            graph.add_clip_with_mask(handle, spec.mask, 1.0, root)
        };
        pose.set_node(node, clip_index, spec.mask);
        nodes.push(node);
    }
    let graph = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    (graph, nodes, pose)
}

fn twins(
    app: &mut App,
    bones: u16,
    graph: &Handle<AnimationGraph>,
    pose: &PoseSource,
) -> (Entity, Vec<Entity>, Entity) {
    let spawn_root = |app: &mut App| {
        app.world_mut()
            .spawn((
                Transform::default(),
                AnimationPlayer::default(),
                AnimationTransitions::new(),
                AnimationGraphHandle(graph.clone()),
            ))
            .id()
    };
    let oracle = spawn_root(app);
    let oracle_bones = (0..bones)
        .map(|i| {
            app.world_mut()
                .spawn((
                    Transform::default(),
                    ChildOf(oracle),
                    target(i),
                    AnimatedBy(oracle),
                ))
                .id()
        })
        .collect();
    let ours = spawn_root(app);
    let skeleton = ModelSkeleton {
        joints: (0..bones)
            .map(|_| ModelJoint {
                parent: -1,
                local_translation: Vec3::ZERO,
                billboard: None,
                parent_arm: None,
            })
            .collect(),
        spine_bone: None,
        head_bone: None,
    };
    let anims = ModelAnimations {
        graph: graph.clone(),
        clips: Vec::new(),
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        pose: Arc::new(pose.clone()),
    };
    app.world_mut()
        .entity_mut(ours)
        .insert((anims, RigPose::new(ours, &skeleton)));
    (oracle, oracle_bones, ours)
}

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), AnimationPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(1)));
    plugin(&mut app);
    app
}

/// Bevy builds its evaluation of a graph the frame after the graph asset arrives, so it poses
/// nothing on the first.
fn warm_up(app: &mut App) {
    app.update();
}

fn step(app: &mut App, secs: f32) {
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
        secs,
    )));
    app.update();
}

fn drive(
    app: &mut App,
    rigs: &[Entity],
    f: impl Fn(&mut AnimationPlayer, &mut AnimationTransitions),
) {
    for &rig in rigs {
        let mut e = app.world_mut().entity_mut(rig);
        let mut transitions = e.take::<AnimationTransitions>().expect("transitions");
        {
            let player = e.get_mut::<AnimationPlayer>().expect("player");
            f(player.into_inner(), &mut transitions);
        }
        e.insert(transitions);
    }
}

#[track_caller]
fn assert_twins(app: &App, oracle_bones: &[Entity], ours: Entity) {
    let rig = app.world().entity(ours).get::<RigPose>().expect("rig");
    for (i, &bone) in oracle_bones.iter().enumerate() {
        let bevy = *app.world().entity(bone).get::<Transform>().expect("bone");
        assert_eq!(bevy, rig.locals[i], "bone {i}");
    }
}

#[test]
fn one_clip_poses_as_bevy_poses_it() {
    let mut app = app();
    let spec = ClipSpec {
        bones: vec![
            (
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::ZERO), (0.4, Vec3::X), (0.8, Vec3::Y)],
                    r: vec![(0.0, Quat::IDENTITY), (0.8, Quat::from_rotation_y(2.0))],
                    s: vec![],
                },
            ),
            (
                1,
                BoneSpec {
                    t: vec![],
                    r: vec![
                        (0.1, Quat::from_rotation_x(0.4)),
                        (0.5, Quat::from_rotation_z(-1.2)),
                    ],
                    s: vec![(0.0, Vec3::ONE), (0.8, Vec3::splat(2.0))],
                },
            ),
        ],
        mask: 0,
    };
    let (graph, nodes, pose) = build(&mut app, &[spec], vec![0, 0, 0]);
    let (oracle, bones, ours) = twins(&mut app, 3, &graph, &pose);
    warm_up(&mut app);
    drive(&mut app, &[oracle, ours], |p, _| {
        p.play(nodes[0]).repeat();
    });
    for _ in 0..24 {
        step(&mut app, 0.05);
        assert_twins(&app, &bones, ours);
    }
}

#[test]
fn a_cross_fade_poses_as_bevy_poses_it() {
    let mut app = app();
    let walk = ClipSpec {
        bones: vec![(
            0,
            BoneSpec {
                t: vec![(0.0, Vec3::ZERO), (0.6, Vec3::X * 3.0)],
                r: vec![(0.0, Quat::IDENTITY), (0.6, Quat::from_rotation_z(1.0))],
                s: vec![],
            },
        )],
        mask: 0,
    };
    let run = ClipSpec {
        bones: vec![(
            0,
            BoneSpec {
                t: vec![(0.0, Vec3::Y), (0.3, Vec3::NEG_Y)],
                r: vec![
                    (0.0, Quat::from_rotation_x(0.5)),
                    (0.3, Quat::from_rotation_x(-0.5)),
                ],
                s: vec![],
            },
        )],
        mask: 0,
    };
    let (graph, nodes, pose) = build(&mut app, &[walk, run], vec![0]);
    let (oracle, bones, ours) = twins(&mut app, 1, &graph, &pose);
    warm_up(&mut app);
    drive(&mut app, &[oracle, ours], |p, tr| {
        tr.play(p, nodes[0], Duration::ZERO).repeat();
    });
    step(&mut app, 0.1);
    drive(&mut app, &[oracle, ours], |p, tr| {
        tr.play(p, nodes[1], Duration::from_secs_f32(0.25)).repeat();
    });
    for _ in 0..6 {
        step(&mut app, 0.06);
        assert_twins(&app, &bones, ours);
    }
}

#[test]
fn a_masked_overlay_poses_as_bevy_poses_it() {
    let mut app = app();
    let base = ClipSpec {
        bones: vec![
            (
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::ZERO), (0.5, Vec3::X)],
                    r: vec![(0.0, Quat::IDENTITY), (0.5, Quat::from_rotation_y(1.0))],
                    s: vec![],
                },
            ),
            (
                1,
                BoneSpec {
                    t: vec![(0.0, Vec3::ZERO), (0.5, Vec3::Z)],
                    r: vec![(0.0, Quat::IDENTITY), (0.5, Quat::from_rotation_z(0.7))],
                    s: vec![],
                },
            ),
        ],
        mask: 0,
    };
    let overlay = ClipSpec {
        bones: vec![
            (
                0,
                BoneSpec {
                    t: vec![(0.0, Vec3::splat(9.0)), (0.4, Vec3::splat(9.0))],
                    r: vec![],
                    s: vec![],
                },
            ),
            (
                1,
                BoneSpec {
                    t: vec![(0.0, Vec3::NEG_Z), (0.4, Vec3::NEG_X)],
                    r: vec![
                        (0.0, Quat::from_rotation_x(1.2)),
                        (0.4, Quat::from_rotation_x(0.2)),
                    ],
                    s: vec![],
                },
            ),
        ],
        mask: 1 << 2,
    };
    let silent = ClipSpec {
        bones: vec![(
            1,
            BoneSpec {
                t: vec![(0.0, Vec3::splat(50.0)), (0.4, Vec3::splat(50.0))],
                r: vec![],
                s: vec![],
            },
        )],
        mask: 0,
    };
    let (graph, nodes, pose) = build(&mut app, &[base, overlay, silent], vec![1 << 2, 0]);
    let (oracle, bones, ours) = twins(&mut app, 2, &graph, &pose);
    warm_up(&mut app);
    drive(&mut app, &[oracle, ours], |p, _| {
        p.play(nodes[0]).repeat();
        p.play(nodes[1]).repeat().set_weight(8.0);
        p.play(nodes[2]).repeat().set_weight(0.0);
    });
    for _ in 0..4 {
        step(&mut app, 0.07);
        assert_twins(&app, &bones, ours);
    }
    let bone0 = app
        .world()
        .entity(ours)
        .get::<RigPose>()
        .expect("rig")
        .locals[0];
    assert_ne!(
        bone0.translation,
        Vec3::splat(9.0),
        "the mask spares bone 0"
    );
}
