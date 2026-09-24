//! The driver run in a headless app over a synthetic body whose clips have real lengths, so the
//! players advance, finish and wrap as the client's clock would.

use std::sync::Arc;
use std::time::Duration;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{ActiveAnimation, AnimationClip, AnimationPlugin};
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::super::motion::anim::{
    FALL, JUMP, JUMP_END, JUMP_LAND_RUN, JUMP_START, RUN, SHUFFLE_LEFT, SIT_GROUND,
    SIT_GROUND_DOWN, SIT_GROUND_UP, STAND, SWIM, SWIM_IDLE, WALK, WALK_BACKWARDS,
};
use super::super::motion::move_flags::{
    BACKWARD, FALLING, FALLING_FAR, FORWARD, SWIMMING, TURN_LEFT, WALK_MODE,
};
use super::super::motion::{Bracketed, Mode, StandState, UnitMotion};
use super::{UnitDriver, drive_units};
use crate::rig::{AnimClip, AnimRng, ModelAnimations};

const STEP: Duration = Duration::from_millis(10);

/// `(id, length s, looping, authored speed, weight, replay)`
type Row = (u16, f32, bool, f32, u16, (u32, u32));

const WALKER: [Row; 12] = [
    (STAND, 2.0, true, 0.0, 0x7fff, (0, 0)),
    (WALK, 1.0, true, 2.5, 0x7fff, (0, 0)),
    (RUN, 0.667, true, 6.944, 0x7fff, (0, 0)),
    (SHUFFLE_LEFT, 0.5, true, 0.0, 0x7fff, (0, 0)),
    (WALK_BACKWARDS, 1.0, true, 2.5, 0x7fff, (0, 0)),
    (JUMP_START, 0.3, false, 0.0, 0x7fff, (0, 0)),
    (JUMP, 1.0, true, 0.0, 0x7fff, (0, 0)),
    (JUMP_END, 0.3, false, 0.0, 0x7fff, (0, 0)),
    (FALL, 1.0, true, 0.0, 0x7fff, (0, 0)),
    (SWIM_IDLE, 1.667, true, 0.0, 0x7fff, (0, 0)),
    (SWIM, 1.0, true, 4.722, 0x7fff, (0, 0)),
    (JUMP_LAND_RUN, 0.3, false, 6.944, 0x7fff, (0, 0)),
];

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), AnimationPlugin))
        .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .init_resource::<AnimRng>()
        .add_systems(Update, drive_units);
    app
}

fn body(app: &mut App, rows: &[Row]) -> Entity {
    let handles: Vec<_> = rows
        .iter()
        .map(|r| {
            let mut c = AnimationClip::default();
            c.set_duration(r.1);
            app.world_mut()
                .resource_mut::<Assets<AnimationClip>>()
                .add(c)
        })
        .collect();
    let (graph, nodes) = AnimationGraph::from_clips(handles);
    let graph = app
        .world_mut()
        .resource_mut::<Assets<AnimationGraph>>()
        .add(graph);
    let clips = rows
        .iter()
        .zip(&nodes)
        .enumerate()
        .map(|(i, (r, &node))| AnimClip {
            anim_id: r.0,
            seq_index: i,
            node,
            looping: r.2,
            duration: r.1,
            move_speed: r.3,
            blend_time: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            frequency: r.4,
            replay: r.5,
            poses_bones: true,
            events: std::sync::Arc::from([]),
        })
        .collect();
    let anims = ModelAnimations {
        graph: graph.clone(),
        clips,
        playable_animation_lookup: Vec::new(),
        animation_lookup: Vec::new(),
        global_bones: Vec::new(),
        moving_idle: None,
        pose: Arc::default(),
    };
    app.world_mut()
        .spawn((
            anims,
            AnimationPlayer::default(),
            AnimationTransitions::new(),
            AnimationGraphHandle(graph),
            UnitDriver::default(),
            UnitMotion::default(),
            Transform::default(),
        ))
        .id()
}

fn moving(app: &mut App, unit: Entity, flags: u32, speed: f32, vertical_speed: f32) {
    app.world_mut().entity_mut(unit).insert(UnitMotion {
        speed,
        vertical_speed,
        flags,
        stand_state: StandState::STAND,
    });
}

fn posed(app: &mut App, unit: Entity, stand_state: StandState) {
    app.world_mut().entity_mut(unit).insert(UnitMotion {
        stand_state,
        ..UnitMotion::default()
    });
}

fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

fn playing(app: &App, unit: Entity) -> (Option<u16>, f32, Mode) {
    let e = app.world().entity(unit);
    let anims = e.get::<ModelAnimations>().expect("animations");
    let node: Option<AnimationNodeIndex> = e
        .get::<AnimationTransitions>()
        .and_then(AnimationTransitions::get_main_animation);
    let id = node.and_then(|n| anims.clips.iter().find(|c| c.node == n).map(|c| c.anim_id));
    let rate = node
        .and_then(|n| e.get::<AnimationPlayer>().and_then(|p| p.animation(n)))
        .map_or(0.0, ActiveAnimation::speed);
    (id, rate, e.get::<UnitDriver>().expect("driver").mode)
}

#[test]
fn a_body_at_rest_stands_and_a_running_one_runs_at_its_speed() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    frames(&mut app, 2);
    assert_eq!(playing(&app, unit).0, Some(STAND));
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 2);
    let (id, rate, mode) = playing(&app, unit);
    assert_eq!((id, mode), (Some(RUN), Mode::Gait));
    assert!((rate - 7.0 / 6.944).abs() < 1e-5, "{rate}");
    moving(&mut app, unit, FORWARD | WALK_MODE, 2.5, 0.0);
    frames(&mut app, 2);
    assert_eq!(playing(&app, unit).0, Some(WALK));
    moving(&mut app, unit, BACKWARD, 4.5, 0.0);
    frames(&mut app, 2);
    let (id, rate, _) = playing(&app, unit);
    assert_eq!(id, Some(WALK_BACKWARDS));
    assert!((rate - 1.8).abs() < 1e-5, "{rate}");
}

#[test]
fn a_jump_starts_hangs_and_lands_running_then_stands_when_the_keys_let_go() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 2);
    moving(&mut app, unit, FORWARD | FALLING, 7.0, 7.96);
    frames(&mut app, 1);
    assert_eq!(
        playing(&app, unit).0,
        Some(JUMP_START),
        "JumpStart on the launch frame"
    );
    assert_eq!(playing(&app, unit).2, Mode::Entering(Bracketed::Jump));
    moving(&mut app, unit, FORWARD | FALLING, 7.0, 3.0);
    frames(&mut app, 40);
    assert_eq!(
        playing(&app, unit).0,
        Some(JUMP),
        "the hang once JumpStart ends"
    );
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(
        playing(&app, unit).0,
        Some(JUMP_LAND_RUN),
        "a running landing"
    );
    assert!(matches!(
        playing(&app, unit).2,
        Mode::Land {
            id: JUMP_LAND_RUN,
            ..
        }
    ));
    moving(&mut app, unit, 0, 0.0, 0.0);
    frames(&mut app, 2);
    assert_eq!(
        playing(&app, unit).0,
        Some(STAND),
        "letting go drops the landing at once"
    );
}

#[test]
fn a_standing_landing_plays_jump_end_and_a_backing_one_none() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    frames(&mut app, 1);
    moving(&mut app, unit, FALLING, 0.0, 7.96);
    frames(&mut app, 50);
    moving(&mut app, unit, 0, 0.0, 0.0);
    frames(&mut app, 1);
    assert_eq!(playing(&app, unit).0, Some(JUMP_END));
    frames(&mut app, 40);
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(STAND), Mode::Gait)
    );
    moving(&mut app, unit, BACKWARD | FALLING, 4.5, 7.96);
    frames(&mut app, 50);
    moving(&mut app, unit, BACKWARD, 4.5, 0.0);
    frames(&mut app, 1);
    assert_eq!(playing(&app, unit).2, Mode::Gait, "no landing clip backing");
    frames(&mut app, 1);
    assert_eq!(
        playing(&app, unit).0,
        Some(WALK_BACKWARDS),
        "the gait picks up a frame on"
    );
}

#[test]
fn a_step_off_holds_its_gait_until_it_falls_far() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 2);
    for vz in [-0.65_f32, -3.0, -7.4] {
        moving(&mut app, unit, FORWARD | FALLING, 7.0, vz);
        frames(&mut app, 1);
        assert_eq!(
            (playing(&app, unit).0, playing(&app, unit).2),
            (Some(RUN), Mode::Gait),
            "vz {vz}"
        );
    }
    moving(&mut app, unit, FORWARD | FALLING | FALLING_FAR, 7.0, -9.0);
    frames(&mut app, 1);
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(FALL), Mode::Looping(Bracketed::Fall))
    );
}

#[test]
fn a_jump_out_of_a_one_frame_step_off_still_jumps() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    moving(&mut app, unit, FORWARD, 7.0, 0.0);
    frames(&mut app, 2);
    moving(&mut app, unit, FORWARD | FALLING, 7.0, -0.65);
    frames(&mut app, 1);
    assert_eq!(playing(&app, unit).2, Mode::Gait);
    moving(&mut app, unit, FORWARD | FALLING, 7.0, 7.96);
    frames(&mut app, 1);
    assert_eq!(playing(&app, unit).2, Mode::Entering(Bracketed::Jump));
}

#[test]
fn a_turn_in_place_shuffles_its_whole_step_and_a_swimmer_strokes() {
    let mut app = app();
    let unit = body(&mut app, &WALKER);
    frames(&mut app, 1);
    moving(&mut app, unit, TURN_LEFT, 0.0, 0.0);
    frames(&mut app, 2);
    assert_eq!(playing(&app, unit).0, Some(SHUFFLE_LEFT));
    moving(&mut app, unit, 0, 0.0, 0.0);
    frames(&mut app, 10);
    assert_eq!(
        playing(&app, unit).0,
        Some(SHUFFLE_LEFT),
        "the step plays out after the turn"
    );
    frames(&mut app, 60);
    assert_eq!(
        playing(&app, unit).0,
        Some(STAND),
        "and hands back to Stand at its end"
    );
    moving(&mut app, unit, SWIMMING, 0.0, 0.0);
    frames(&mut app, 2);
    assert_eq!(playing(&app, unit).0, Some(SWIM_IDLE));
    moving(&mut app, unit, SWIMMING | FORWARD, 4.722, 0.0);
    frames(&mut app, 2);
    let (id, rate, _) = playing(&app, unit);
    assert_eq!(id, Some(SWIM));
    assert!((rate - 1.0).abs() < 1e-5, "{rate}");
}

#[test]
fn a_standing_body_rolls_among_its_stand_variations() {
    let mut app = app();
    let unit = body(
        &mut app,
        &[
            (STAND, 0.1, true, 0.0, 0x4000, (1, 1)),
            (STAND, 0.1, true, 0.0, 0x4000, (1, 1)),
        ],
    );
    let nodes: Vec<AnimationNodeIndex> = app
        .world()
        .entity(unit)
        .get::<ModelAnimations>()
        .expect("animations")
        .clips
        .iter()
        .map(|c| c.node)
        .collect();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..200 {
        frames(&mut app, 1);
        let e = app.world().entity(unit);
        if let Some(n) = e
            .get::<AnimationTransitions>()
            .and_then(AnimationTransitions::get_main_animation)
        {
            seen.insert(n);
        }
    }
    assert!(nodes.iter().all(|n| seen.contains(n)), "{seen:?}");
}

#[test]
fn a_sit_goes_down_holds_and_stands_up_or_walks_straight_out() {
    let mut app = app();
    let mut rows = WALKER.to_vec();
    rows.extend([
        (SIT_GROUND_DOWN, 0.5, false, 0.0, 0x7fff, (0, 0)),
        (SIT_GROUND, 2.0, true, 0.0, 0x7fff, (0, 0)),
        (SIT_GROUND_UP, 0.5, false, 0.0, 0x7fff, (0, 0)),
    ]);
    let unit = body(&mut app, &rows);
    frames(&mut app, 2);
    let sitting = Bracketed::Pose(StandState::SIT);
    posed(&mut app, unit, StandState::SIT);
    frames(&mut app, 2);
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(SIT_GROUND_DOWN), Mode::Entering(sitting))
    );
    frames(&mut app, 60);
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(SIT_GROUND), Mode::Looping(sitting))
    );
    posed(&mut app, unit, StandState::STAND);
    frames(&mut app, 2);
    let standing_up = Mode::StandingUp {
        pose: StandState::SIT,
        clip: SIT_GROUND_UP,
    };
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(SIT_GROUND_UP), standing_up)
    );
    frames(&mut app, 60);
    assert_eq!(playing(&app, unit).0, Some(STAND));
    posed(&mut app, unit, StandState::SIT);
    frames(&mut app, 70);
    moving(&mut app, unit, FORWARD, 2.5, 0.0);
    frames(&mut app, 2);
    assert_eq!(
        (playing(&app, unit).0, playing(&app, unit).2),
        (Some(WALK), Mode::Gait),
        "walking out of a sit skips the stand-up"
    );
}
