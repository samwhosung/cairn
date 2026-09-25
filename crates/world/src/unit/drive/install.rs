use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::animation::AnimationPlugin;
use bevy::animation::graph::AnimationGraphHandle;
use bevy::animation::transition::AnimationTransitions;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::super::motion::anim::RUN;
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

fn sampled(
    app: &mut App,
    (skeleton, anims): (&ModelSkeleton, &ModelAnimations),
    motion_from_the_telling: &dyn Fn(usize) -> UnitMotion,
    told: Option<u16>,
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
            UnitShow::default(),
            Transform::default(),
        ))
        .id();
    let rig = RigPose::new(body, skeleton);
    app.world_mut().entity_mut(body).insert(rig);
    for _ in 0..SETTLE_FRAMES {
        app.update();
    }
    *app.world_mut().resource_mut::<AnimRng>() = AnimRng::default();
    app.world_mut().entity_mut(body).insert(UnitShow {
        play: told,
        pose: None,
    });
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
                .filter(|c| Some(c.anim_id) == told)
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
    let swing = Some(ATTACK_UNARMED);
    let human = (&skeleton, &anims);
    let whole_body_human = (&skeleton, &whole_body_only);
    let (running, standing) = (&|_| running, &|_| standing);
    let run = sampled(&mut app, human, running, None, frames).locals_from_the_telling;
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
    let swing = Some(ATTACK_UNARMED);
    let human = (&skeleton, &anims);
    let run = sampled(&mut app, human, stand_then_run, None, frames).locals_from_the_telling;
    let moved_up = sampled(&mut app, human, stand_then_run, swing, frames);
    let whole_body_human = (&skeleton, &whole_body_only);
    let today =
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
                today[f][b], swing_whole[f][b],
                "today, the legs stay in the swing"
            );
        }
    }
    let control_off_the_run = legs
        .iter()
        .filter(|&&b| today[legs_ran_into_the_run][b] != run[legs_ran_into_the_run][b])
        .count();
    assert!(
        control_off_the_run > 0,
        "today's whole-body swing keeps the legs off the run"
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
         today's stay the swing's to its end ({control_off_the_run} off the run at frame \
         {legs_ran_into_the_run}); the {} upper bones the swing turns follow it to its end, each \
         at most {worst_share:.4} of the way back to the run",
        legs.len(),
        upper.len()
    );
}
