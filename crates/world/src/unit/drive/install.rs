//! The driver and the pose evaluator over the install's human, bone by bone: a swing on the run
//! leaves the legs to the run and the upper body to the swing, and a swing standing still is the
//! whole-body clip it always was.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::animation::AnimationPlugin;
use bevy::animation::graph::AnimationGraphHandle;
use bevy::animation::transition::AnimationTransitions;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::super::motion::move_flags::FORWARD;
use super::super::motion::{UnitMotion, UnitShow};
use super::{UPPER_BODY_WEIGHT, UnitDriver, drive_units};
use crate::M2Model;
use crate::rig::{AnimRng, ModelAnimations, ModelSkeleton, RigPose, pose_evaluation};

const HUMAN_MALE: &str = "Character\\Human\\Male\\HumanMale.mdx";
const ATTACK_UNARMED: u16 = 16;
const STEP: Duration = Duration::from_nanos(16_666_667);
const SETTLE_FRAMES: usize = 60;
/// The swing's second, its release and a margin.
const SWING_FRAMES: usize = 75;
const MID_SWING: usize = 30;
/// Past the swing's blend in and short of its end, where it shows whole.
const SWING_SHOWN_WHOLE: std::ops::Range<usize> = 10..58;
/// Far enough apart that the swing's own pose shows.
const VISIBLY_APART: f32 = 0.1;

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

/// A body that moves as `motion` and, when `swings`, is told the swing once its gait has settled:
/// its bone locals every frame from that one on, and the sequence that swung.
fn sampled(
    app: &mut App,
    skeleton: &ModelSkeleton,
    anims: &ModelAnimations,
    motion: UnitMotion,
    swings: bool,
) -> (Vec<Vec<Transform>>, Option<usize>) {
    *app.world_mut().resource_mut::<AnimRng>() = AnimRng::default();
    let body = app
        .world_mut()
        .spawn((
            anims.clone(),
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(anims.graph.clone()),
            UnitDriver::default(),
            motion,
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
    if swings {
        app.world_mut().entity_mut(body).insert(UnitShow {
            play: Some(ATTACK_UNARMED),
            pose: None,
        });
    }
    let mut locals = Vec::new();
    let mut swung = None;
    for _ in 0..SWING_FRAMES {
        app.update();
        let e = app.world().entity(body);
        locals.push(e.get::<RigPose>().expect("a rig").locals.clone());
        let player = e.get::<AnimationPlayer>().expect("a player");
        swung = swung.or_else(|| {
            anims
                .clips
                .iter()
                .filter(|c| c.anim_id == ATTACK_UNARMED)
                .find(|c| {
                    std::iter::once(c.node)
                        .chain(c.upper_node)
                        .any(|n| player.animation(n).is_some())
                })
                .map(|c| c.seq_index)
        });
    }
    app.world_mut().entity_mut(body).despawn();
    (locals, swung)
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
    let mut whole_body_only = anims.clone();
    for c in &mut whole_body_only.clips {
        c.upper_node = None;
    }
    let running = UnitMotion {
        speed: 7.0,
        flags: FORWARD,
        ..UnitMotion::default()
    };
    let standing = UnitMotion::default();
    let (run, _) = sampled(&mut app, &skeleton, &anims, running, false);
    let (swing_on_the_run, swung) = sampled(&mut app, &skeleton, &anims, running, true);
    let (whole_on_the_run, _) = sampled(&mut app, &skeleton, &whole_body_only, running, true);
    let (swing_standing, swung_standing) = sampled(&mut app, &skeleton, &anims, standing, true);
    let (whole_standing, _) = sampled(&mut app, &skeleton, &whole_body_only, standing, true);
    assert!(
        swung.is_some() && swung == swung_standing,
        "one swing to compare: {swung:?} and {swung_standing:?}"
    );

    assert!(
        swing_standing == whole_standing,
        "standing still, the swing is the whole-body clip, pose for pose"
    );
    assert!(
        swing_on_the_run != whole_on_the_run,
        "the same comparison sees the swing on the run leave the whole body"
    );

    let src = &anims.pose;
    let lower: Vec<usize> = (0..src.bone_masks.len())
        .filter(|&b| src.bone_masks[b] != 0)
        .collect();
    for (frame, (swinging, running)) in swing_on_the_run.iter().zip(&run).enumerate() {
        for &bone in &lower {
            assert_eq!(
                swinging[bone], running[bone],
                "frame {frame}: bone {bone} of the legs left the run"
            );
        }
    }
    let legs_off_the_run = |frames: &[Vec<Transform>]| {
        lower
            .iter()
            .filter(|&&b| frames[MID_SWING][b] != run[MID_SWING][b])
            .count()
    };
    let control_moved = legs_off_the_run(&whole_on_the_run);
    assert!(
        control_moved > 0,
        "the whole-body swing moves the legs off the run"
    );

    let clip = anims
        .clips
        .iter()
        .find(|c| Some(c.seq_index) == swung)
        .expect("the swing");
    let keyed = &src.clips[src.node(clip.node).expect("its node").clip as usize];
    let upper: Vec<usize> = keyed
        .bones
        .iter()
        .filter(|b| !b.rotation.is_empty())
        .map(|b| usize::from(b.bone))
        .filter(|&b| src.bone_masks[b] == 0)
        .collect();
    let gait_share = 1.0 / (1.0 + UPPER_BODY_WEIGHT);
    let (mut apart_mid_swing, mut worst_share) = (0, 0.0_f32);
    for frame in SWING_SHOWN_WHOLE {
        for &bone in &upper {
            let swing = swing_standing[frame][bone].rotation;
            let from_the_run = angle(run[frame][bone].rotation, swing);
            let from_the_swing = angle(swing_on_the_run[frame][bone].rotation, swing);
            assert!(
                from_the_swing <= from_the_run * gait_share + 1e-4,
                "frame {frame}: bone {bone} stands {from_the_swing} rad off the swing, the run \
                 {from_the_run}"
            );
            if from_the_run > VISIBLY_APART {
                worst_share = worst_share.max(from_the_swing / from_the_run);
                apart_mid_swing += usize::from(frame == MID_SWING);
            }
        }
    }
    assert!(
        apart_mid_swing >= 5,
        "the swing shows above the lower spine: {apart_mid_swing} bones"
    );
    eprintln!(
        "legs: {} bones, the run's bit for bit through the swing (the whole-body control moves {} \
         off it); upper body: {} bones the swing turns, {} of them over {VISIBLY_APART} rad from \
         the run half way through, each at most {worst_share:.4} of the way back to the run",
        lower.len(),
        control_moved,
        upper.len(),
        apart_mid_swing,
    );
}
