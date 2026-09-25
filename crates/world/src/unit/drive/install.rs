use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::animation::AnimationPlugin;
use bevy::animation::graph::AnimationGraphHandle;
use bevy::animation::transition::AnimationTransitions;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::super::motion::anim::{COMBAT_WOUND, READY_UNARMED, RUN};
use super::super::motion::move_flags::FORWARD;
use super::super::motion::{UnitMotion, UnitShow};
use super::{UPPER_BODY_OVER_GAIT, UPPER_BODY_RELEASE_SECS, UnitDriver, drive_units};
use crate::M2Model;
use crate::rig::{AnimRng, ModelAnimations, ModelSkeleton, RigPose, pose_evaluation};

const HUMAN_MALE: &str = "Character\\Human\\Male\\HumanMale.mdx";
const ATTACK_UNARMED: u16 = 16;
const STEP: Duration = Duration::from_nanos(16_666_667);
const SETTLE_FRAMES: usize = 60;
const VISIBLY_APART_RAD: f32 = 0.1;

fn app(data: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let install = crate::Install::open(data).expect("open the install");
    crate::register_source(&mut app, &install);
    app.add_plugins((AssetPlugin::default(), AnimationPlugin))
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .add_plugins(crate::LoadersPlugin)
        .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .init_resource::<AnimRng>()
        .add_systems(Update, drive_units);
    pose_evaluation(&mut app);
    app.finish();
    app.cleanup();
    app
}

fn human(app: &mut App) -> (ModelSkeleton, ModelAnimations) {
    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<M2Model> = server.load(crate::m2_url(HUMAN_MALE));
    let deadline = Instant::now() + Duration::from_secs(120);
    while matches!(server.load_state(&handle), LoadState::Loading) {
        assert!(Instant::now() < deadline, "the human never loaded");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
    let m = app
        .world()
        .resource::<Assets<M2Model>>()
        .get(&handle)
        .expect("the human loads");
    (
        m.skeleton.clone(),
        m.animations.clone().expect("its sequences"),
    )
}

fn frames_in(secs: f32) -> usize {
    (secs / STEP.as_secs_f32()).ceil() as usize
}

struct Sampled {
    locals_from_the_telling: Vec<Vec<Transform>>,
    swung_seq: Option<usize>,
}

/// What a body is shown from its spawn, and what it is told once its gait has settled.
#[derive(Clone, Copy, Default)]
struct Scene {
    from_the_spawn: UnitShow,
    told: UnitShow,
}

impl Scene {
    fn plays(play: Option<u16>) -> Self {
        Self {
            told: UnitShow {
                play,
                ..UnitShow::default()
            },
            ..Self::default()
        }
    }
}

fn sampled(
    app: &mut App,
    (skeleton, anims): (&ModelSkeleton, &ModelAnimations),
    motion_from_the_telling: &dyn Fn(usize) -> UnitMotion,
    scene: Scene,
    frames: usize,
) -> Sampled {
    *app.world_mut().resource_mut::<AnimRng>() = AnimRng::default();
    let body = app
        .world_mut()
        .spawn((
            anims.clone(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(anims.graph.clone()),
            UnitDriver::default(),
            motion_from_the_telling(0),
            scene.from_the_spawn,
            Transform::default(),
        ))
        .id();
    let rig = RigPose::new(body, skeleton);
    app.world_mut().entity_mut(body).insert(rig);
    for _ in 0..SETTLE_FRAMES {
        app.update();
    }
    *app.world_mut().resource_mut::<AnimRng>() = AnimRng::default();
    app.world_mut().entity_mut(body).insert(scene.told);
    let mut locals = Vec::new();
    let mut swung_seq = None;
    for frame in 0..frames {
        app.world_mut()
            .entity_mut(body)
            .insert(motion_from_the_telling(frame));
        app.update();
        let e = app.world().entity(body);
        locals.push(e.get::<RigPose>().expect("a rig").locals.clone());
        let player = e.get::<AnimationPlayer>().expect("a player");
        swung_seq = swung_seq.or_else(|| {
            anims
                .clips
                .iter()
                .filter(|c| Some(c.anim_id) == scene.told.play)
                .find(|c| {
                    std::iter::once(c.node)
                        .chain(c.upper_node)
                        .any(|n| player.animation(n).is_some())
                })
                .map(|c| c.seq_index)
        });
    }
    app.world_mut().entity_mut(body).despawn();
    Sampled {
        locals_from_the_telling: locals,
        swung_seq,
    }
}

fn without_upper_nodes(anims: &ModelAnimations) -> ModelAnimations {
    let mut whole_body_only = anims.clone();
    for c in &mut whole_body_only.clips {
        c.upper_node = None;
    }
    whole_body_only
}

fn run_moves_below_the_spine(anims: &ModelAnimations) -> Vec<usize> {
    let src = &anims.pose;
    let run = anims.find_resolved(RUN, &|_| None).expect("a run");
    let keyed = &src.clips[src.node(run.node).expect("its node").clip as usize];
    keyed
        .bones
        .iter()
        .map(|b| usize::from(b.bone))
        .filter(|&b| src.bone_masks[b] != 0)
        .collect()
}

fn below_the_spine(anims: &ModelAnimations) -> Vec<usize> {
    let masks = &anims.pose.bone_masks;
    (0..masks.len()).filter(|&b| masks[b] != 0).collect()
}

fn turned_above_the_spine(anims: &ModelAnimations, seq: Option<usize>) -> Vec<usize> {
    let src = &anims.pose;
    let clip = anims
        .clips
        .iter()
        .find(|c| Some(c.seq_index) == seq)
        .expect("the clip");
    let keyed = &src.clips[src.node(clip.node).expect("its node").clip as usize];
    keyed
        .bones
        .iter()
        .filter(|b| !b.rotation.is_empty())
        .map(|b| usize::from(b.bone))
        .filter(|&b| src.bone_masks[b] == 0)
        .collect()
}

fn angle(a: Quat, b: Quat) -> f32 {
    let d = a.inverse() * b;
    2.0 * d.xyz().length().atan2(d.w.abs())
}

#[test]
fn a_swing_on_the_run_leaves_the_legs_to_the_run_and_one_standing_takes_the_whole_body() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut app = app(&data);
    let (skeleton, anims) = human(&mut app);
    let whole_body_only = without_upper_nodes(&anims);
    let swings = || anims.clips.iter().filter(|c| c.anim_id == ATTACK_UNARMED);
    let swing_secs = swings().map(|c| c.duration).fold(0.0, f32::max);
    let blend_secs = swings().map(|c| c.blend_time).fold(0.0, f32::max);
    let frames = frames_in(swing_secs + UPPER_BODY_RELEASE_SECS) + 5;
    let mid_swing = frames_in(swing_secs / 2.0);
    let swing_shown_whole = frames_in(blend_secs) + 1..frames_in(swing_secs) - 2;
    let running = UnitMotion {
        speed: 7.0,
        flags: FORWARD,
        ..UnitMotion::default()
    };
    let standing = UnitMotion::default();
    let (swing, bare) = (Scene::plays(Some(ATTACK_UNARMED)), Scene::default());
    let human = (&skeleton, &anims);
    let whole_body_human = (&skeleton, &whole_body_only);
    let (running, standing) = (&|_| running, &|_| standing);
    let run = sampled(&mut app, human, running, bare, frames).locals_from_the_telling;
    let on_the_run = sampled(&mut app, human, running, swing, frames);
    let whole_on_the_run =
        sampled(&mut app, whole_body_human, running, swing, frames).locals_from_the_telling;
    let standing_still = sampled(&mut app, human, standing, swing, frames);
    let whole_standing =
        sampled(&mut app, whole_body_human, standing, swing, frames).locals_from_the_telling;
    let swing_on_the_run = &on_the_run.locals_from_the_telling;
    let swing_standing = &standing_still.locals_from_the_telling;
    assert!(
        on_the_run.swung_seq.is_some() && on_the_run.swung_seq == standing_still.swung_seq,
        "one swing to compare: {:?} and {:?}",
        on_the_run.swung_seq,
        standing_still.swung_seq
    );

    assert!(
        *swing_standing == whole_standing,
        "standing still, the swing is the whole-body clip, pose for pose"
    );
    assert!(
        *swing_on_the_run != whole_on_the_run,
        "the same comparison sees the swing on the run leave the whole body"
    );

    let lower = below_the_spine(&anims);
    for (frame, (swinging, running)) in swing_on_the_run.iter().zip(&run).enumerate() {
        for &bone in &lower {
            assert_eq!(
                swinging[bone], running[bone],
                "frame {frame}: bone {bone} of the legs left the run"
            );
        }
    }
    let control_moved = lower
        .iter()
        .filter(|&&b| whole_on_the_run[mid_swing][b] != run[mid_swing][b])
        .count();
    assert!(
        control_moved > 0,
        "the whole-body swing moves the legs off the run"
    );

    let upper = turned_above_the_spine(&anims, on_the_run.swung_seq);
    let gait_share = 1.0 / (1.0 + UPPER_BODY_OVER_GAIT);
    let (mut apart_mid_swing, mut worst_share) = (0, 0.0_f32);
    for frame in swing_shown_whole {
        for &bone in &upper {
            let swing = swing_standing[frame][bone].rotation;
            let from_the_run = angle(run[frame][bone].rotation, swing);
            let from_the_swing = angle(swing_on_the_run[frame][bone].rotation, swing);
            assert!(
                from_the_swing <= from_the_run * gait_share + 1e-4,
                "frame {frame}: bone {bone} stands {from_the_swing} rad off the swing, the run \
                 {from_the_run}"
            );
            if from_the_run > VISIBLY_APART_RAD {
                worst_share = worst_share.max(from_the_swing / from_the_run);
                apart_mid_swing += usize::from(frame == mid_swing);
            }
        }
    }
    assert!(
        apart_mid_swing >= 5,
        "the swing shows above the lower spine: {apart_mid_swing} bones"
    );
    eprintln!(
        "legs: {} bones, the run's bit for bit through {frames} frames (the whole-body control \
         moves {control_moved} off it half way); upper body: {} bones the swing turns, \
         {apart_mid_swing} of them over {VISIBLY_APART_RAD} rad from the run half way through, \
         each at most {worst_share:.4} of the way back to the run",
        lower.len(),
        upper.len(),
    );
}

#[test]
fn a_swing_begun_standing_moves_up_when_the_body_runs_and_the_legs_take_the_run() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut app = app(&data);
    let (skeleton, anims) = human(&mut app);
    let whole_body_only = without_upper_nodes(&anims);
    let swings = || anims.clips.iter().filter(|c| c.anim_id == ATTACK_UNARMED);
    let swing_secs = swings().map(|c| c.duration).fold(0.0, f32::max);
    let run_blend_secs = anims
        .find_resolved(RUN, &|_| None)
        .expect("a run")
        .blend_time;
    let frames = frames_in(swing_secs + UPPER_BODY_RELEASE_SECS) + 5;
    let runs_from = frames_in(swing_secs * 0.3);
    let legs_ran_into_the_run = runs_from + frames_in(run_blend_secs);
    let swing_ends = frames_in(swing_secs) - 2;
    let running = UnitMotion {
        speed: 7.0,
        flags: FORWARD,
        ..UnitMotion::default()
    };
    let standing = UnitMotion::default();
    let stand_then_run = &|frame| if frame < runs_from { standing } else { running };
    let (swing, bare) = (Scene::plays(Some(ATTACK_UNARMED)), Scene::default());
    let human = (&skeleton, &anims);
    let run = sampled(&mut app, human, stand_then_run, bare, frames).locals_from_the_telling;
    let moved_up = sampled(&mut app, human, stand_then_run, swing, frames);
    let whole_body_human = (&skeleton, &whole_body_only);
    let never_lifted =
        sampled(&mut app, whole_body_human, stand_then_run, swing, frames).locals_from_the_telling;
    let standing_still = sampled(&mut app, human, &|_| standing, swing, frames);
    assert!(
        moved_up.swung_seq.is_some() && moved_up.swung_seq == standing_still.swung_seq,
        "one swing to compare"
    );
    let (moved_up, swing_whole) = (
        &moved_up.locals_from_the_telling,
        &standing_still.locals_from_the_telling,
    );
    assert!(
        moved_up[..runs_from] == swing_whole[..runs_from],
        "until it runs, the swing is the whole-body clip"
    );

    let legs = run_moves_below_the_spine(&anims);
    let legs_on_the_run =
        |frames: &[Vec<Transform>], f: usize| legs.iter().all(|&b| frames[f][b] == run[f][b]);
    let first_on_the_run = (runs_from..frames).find(|&f| legs_on_the_run(moved_up, f));
    for f in legs_ran_into_the_run..frames {
        assert!(
            legs_on_the_run(moved_up, f),
            "frame {f}: the legs left the run"
        );
    }
    for f in runs_from..swing_ends {
        for &b in &legs {
            assert_eq!(
                never_lifted[f][b], swing_whole[f][b],
                "never lifted, the legs stay in the swing"
            );
        }
    }
    let control_off_the_run = legs
        .iter()
        .filter(|&&b| never_lifted[legs_ran_into_the_run][b] != run[legs_ran_into_the_run][b])
        .count();
    assert!(
        control_off_the_run > 0,
        "a whole-body swing never lifted keeps the legs off the run"
    );

    let upper = turned_above_the_spine(&anims, standing_still.swung_seq);
    let gait_share = 1.0 / (1.0 + UPPER_BODY_OVER_GAIT);
    let mut worst_share = 0.0_f32;
    for f in legs_ran_into_the_run..swing_ends {
        for &bone in &upper {
            let swing = swing_whole[f][bone].rotation;
            let from_the_run = angle(run[f][bone].rotation, swing);
            let from_the_swing = angle(moved_up[f][bone].rotation, swing);
            assert!(
                from_the_swing <= from_the_run * gait_share + 1e-4,
                "frame {f}: bone {bone} stands {from_the_swing} rad off the swing, the run \
                 {from_the_run}"
            );
            if from_the_run > VISIBLY_APART_RAD {
                worst_share = worst_share.max(from_the_swing / from_the_run);
            }
        }
    }
    eprintln!(
        "runs from frame {runs_from}; the {} leg bones the run moves are its own bit for bit from \
         frame {first_on_the_run:?} (asserted from {legs_ran_into_the_run}) to {frames}, and \
         a swing never lifted keeps them to its end ({control_off_the_run} off the run at frame \
         {legs_ran_into_the_run}); the {} upper bones the swing turns follow it to its end, each \
         at most {worst_share:.4} of the way back to the run",
        legs.len(),
        upper.len()
    );
}

fn the_clients_wound_share(weighed_at: f32, span: f32) -> f32 {
    let fraction_left = (1.0 - weighed_at / span).clamp(0.0, 1.0);
    (3.0 - 2.0 * fraction_left) * fraction_left * fraction_left * 0.75
}

#[test]
fn a_wound_begun_standing_leaves_the_legs_to_the_run_and_eases_out_over_the_torso() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut app = app(&data);
    let (skeleton, anims) = human(&mut app);
    let wound = anims
        .find_resolved(COMBAT_WOUND, &|_| None)
        .expect("a wound");
    let frames = frames_in(wound.duration) + 10;
    let runs_from = frames_in(wound.duration * 0.3);
    let running = UnitMotion {
        speed: 7.0,
        flags: FORWARD,
        ..UnitMotion::default()
    };
    let standing = UnitMotion::default();
    let stand_then_run = &|frame| if frame < runs_from { standing } else { running };
    let human = (&skeleton, &anims);
    let run =
        sampled(&mut app, human, stand_then_run, Scene::default(), frames).locals_from_the_telling;
    let hit = sampled(
        &mut app,
        human,
        stand_then_run,
        Scene::plays(Some(COMBAT_WOUND)),
        frames,
    );
    let hit = &hit.locals_from_the_telling;

    let legs = below_the_spine(&anims);
    for (f, (hit, run)) in hit.iter().zip(&run).enumerate() {
        for &b in &legs {
            assert_eq!(
                hit[b], run[b],
                "frame {f}: bone {b} of the legs left the run"
            );
        }
    }

    let src = &anims.pose;
    let keyed = &src.clips[src.node(wound.node).expect("its node").clip as usize];
    let dt = STEP.as_secs_f32();
    let (mut worst_off, mut shown) = (0.0_f32, 0);
    for f in 0..frames_in(wound.duration) - 1 {
        let (weighed_at, sampled_at) = (f as f32 * dt, (f + 1) as f32 * dt);
        let share = the_clients_wound_share(weighed_at, wound.duration);
        for bone in keyed
            .bones
            .iter()
            .filter(|b| src.bone_masks[usize::from(b.bone)] == 0)
        {
            let Some(flinch) = bone.rotation.sample(sampled_at) else {
                continue;
            };
            let b = usize::from(bone.bone);
            let expected = run[f][b].rotation.slerp(flinch, share);
            let off = angle(hit[f][b].rotation, expected);
            worst_off = worst_off.max(off);
            assert!(
                off < 2e-3,
                "frame {f}: bone {b} stands {off} rad off the client's wound, share {share}"
            );
            shown += usize::from(angle(run[f][b].rotation, hit[f][b].rotation) > VISIBLY_APART_RAD);
        }
    }
    assert!(shown > 0, "the wound shows above the spine");
    eprintln!(
        "the {} bones below the spine are the run's bit for bit on all {frames} frames, standing \
         and running from frame {runs_from}; above it, the wound's share of the pose is \
         the client's to within {worst_off:.2e} rad, {shown} bone-frames over {VISIBLY_APART_RAD} \
         rad from the run",
        legs.len(),
    );
}

fn bone_frames_apart(a: &[Vec<Transform>], b: &[Vec<Transform>]) -> usize {
    a.iter()
        .zip(b)
        .map(|(a, b)| a.iter().zip(b).filter(|(a, b)| a != b).count())
        .sum()
}

#[test]
fn an_idled_body_stands_ready_bone_for_bone_runs_as_it_would_and_stands_once_let_go() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut app = app(&data);
    let (skeleton, anims) = human(&mut app);
    let ready = anims
        .find_resolved(READY_UNARMED, &|_| None)
        .expect("a ready stance");
    let frames = frames_in(ready.duration * 2.0) + 10;
    let running = UnitMotion {
        speed: 7.0,
        flags: FORWARD,
        ..UnitMotion::default()
    };
    let (running, standing) = (&|_| running, &|_| UnitMotion::default());
    let idles = UnitShow {
        idle: Some(READY_UNARMED),
        ..UnitShow::default()
    };
    let holds = UnitShow {
        pose: Some(READY_UNARMED),
        ..UnitShow::default()
    };
    let told = |told| Scene {
        told,
        ..Scene::default()
    };
    let let_go = |from_the_spawn| Scene {
        from_the_spawn,
        ..Scene::default()
    };
    let mut body = |motion: &dyn Fn(usize) -> UnitMotion, scene| {
        sampled(&mut app, (&skeleton, &anims), motion, scene, frames).locals_from_the_telling
    };
    let idled = body(standing, told(idles));
    let held = body(standing, told(holds));
    let dropped = body(standing, told(UnitShow::default()));
    let runs = body(running, Scene::default());
    let runs_idled = body(running, told(idles));
    let let_go_of_the_idle = body(standing, let_go(idles));
    let let_go_of_the_pose = body(standing, let_go(holds));
    let kept = body(
        standing,
        Scene {
            from_the_spawn: idles,
            told: idles,
        },
    );

    assert!(
        idled == held,
        "idled, the body stands in the stance as one holding it does, bone for bone"
    );
    let apart = bone_frames_apart(&dropped, &held);
    assert!(
        apart > 0,
        "its show dropped, the body stands in Stand, which the comparison tells apart"
    );
    assert!(runs_idled == runs, "moving, it runs as it would");
    assert!(
        let_go_of_the_idle == let_go_of_the_pose,
        "let go, it stands as a body let go of a held pose does"
    );
    let kept_apart = bone_frames_apart(&kept, &let_go_of_the_pose);
    assert!(kept_apart > 0, "the comparison tells a stance kept apart");
    eprintln!(
        "over {frames} frames of {} bones: idled, the stance held, bit for bit (dropped, {apart} \
         bone-frames apart); running, the run bit for bit; let go, a let-go pose's stand bit for \
         bit (kept, {kept_apart} apart)",
        skeleton.joints.len()
    );
}

#[test]
fn a_wound_over_the_ready_stance_takes_the_whole_body_and_one_without_it_the_torso() {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return;
    };
    let mut app = app(&data);
    let (skeleton, anims) = human(&mut app);
    let whole_body_only = without_upper_nodes(&anims);
    let wound = anims
        .find_resolved(COMBAT_WOUND, &|_| None)
        .expect("a wound");
    let frames = frames_in(wound.duration) + 10;
    let standing = &|_| UnitMotion::default();
    let ready = UnitShow {
        idle: Some(READY_UNARMED),
        ..UnitShow::default()
    };
    let hit = UnitShow {
        play: Some(COMBAT_WOUND),
        ..ready
    };
    let (in_the_stance, stance_left_out) = (
        Scene {
            from_the_spawn: ready,
            told: hit,
        },
        Scene::plays(Some(COMBAT_WOUND)),
    );
    let mut body = |anims, scene| {
        sampled(&mut app, (&skeleton, anims), standing, scene, frames).locals_from_the_telling
    };
    let recoils = body(&anims, in_the_stance);
    let recoils_whole = body(&whole_body_only, in_the_stance);
    let stands_ready = body(
        &anims,
        Scene {
            from_the_spawn: ready,
            told: ready,
        },
    );
    let flinches = body(&anims, stance_left_out);
    let flinches_whole = body(&whole_body_only, stance_left_out);
    let stands = body(&anims, Scene::default());

    assert!(
        recoils == recoils_whole,
        "over the stance, the wound is its whole-body clip's, bone for bone"
    );
    let control_apart = bone_frames_apart(&flinches, &flinches_whole);
    assert!(
        control_apart > 0,
        "the stance left out, the comparison tells the wound apart from its whole-body clip"
    );
    let legs = below_the_spine(&anims);
    let legs_moved = |a: &[Vec<Transform>], b: &[Vec<Transform>]| {
        a.iter()
            .zip(b)
            .map(|(a, b)| legs.iter().filter(|&&l| a[l] != b[l]).count())
            .sum::<usize>()
    };
    let recoiled = legs_moved(&recoils, &stands_ready);
    assert!(recoiled > 0, "over the stance, the wound moves the legs");
    assert_eq!(
        legs_moved(&flinches, &stands),
        0,
        "without it, the legs stand in Stand bit for bit"
    );
    eprintln!(
        "over {frames} frames: over the stance, the wound is the whole-body clip's bit for bit and \
         moves the {} leg bones on {recoiled} bone-frames; without it, the legs are the Stand's \
         bit for bit and the whole-body comparison is {control_apart} bone-frames apart",
        legs.len()
    );
}
