//! The event keys a playing clip crosses, fired where they stand in the world.

use bevy::animation::AnimationPlayer;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;

use crate::rig::{AnimClip, ClipEvent, ModelAnimations, RigPose};

/// One event key a clip crossed this frame.
#[derive(Message, Clone, Copy, Debug)]
pub struct AnimEvent {
    pub entity: Entity,
    pub ident: [u8; 4],
    pub data: u32,
    /// The clip's `AnimationData.dbc` id.
    pub anim_id: u16,
    /// Where the key fired, world space: its bone's live pose when the model has one, else its
    /// model-space point through the model's transform.
    pub pos: Vec3,
}

/// A footfall's sound key; the per-foot side keys are its visuals and sound nothing.
pub fn is_footstep_sound(ident: &[u8; 4]) -> bool {
    ident == b"$FSD"
}

pub(crate) struct EventFrame<'a> {
    pub(crate) world: &'a GlobalTransform,
    pub(crate) rig: Option<(&'a RigPose, &'a GlobalTransform)>,
}

impl EventFrame<'_> {
    fn point(&self, e: &ClipEvent) -> Vec3 {
        self.rig
            .and_then(|(pose, root)| pose.posed_point(root, e.bone, e.offset))
            .unwrap_or_else(|| self.world.transform_point(e.point))
    }
}

const HEAD_KEYS_ARM_WINDOW_SECS: f32 = 0.25;

#[derive(Clone, Copy)]
pub(crate) struct TrackSeek {
    node: AnimationNodeIndex,
    seek: f32,
    armed: bool,
}

pub(crate) type TrackMemory = EntityHashMap<TrackSeek>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TrackScan {
    Arming,
    FromHead,
    After(f32),
}

pub(crate) fn advance_track(
    last: &mut TrackMemory,
    entity: Entity,
    node: AnimationNodeIndex,
    cur: f32,
) -> TrackScan {
    let was = last.get(&entity).copied().filter(|t| t.node == node);
    last.insert(
        entity,
        TrackSeek {
            node,
            seek: cur,
            armed: was.is_none(),
        },
    );
    match was {
        None => TrackScan::Arming,
        Some(was) if was.armed && was.seek <= HEAD_KEYS_ARM_WINDOW_SECS => TrackScan::FromHead,
        Some(was) => TrackScan::After(was.seek),
    }
}

/// A wrap fires the old pass's tail, then the new one's head.
pub(crate) fn scan_events(
    clip: &AnimClip,
    entity: Entity,
    scan: TrackScan,
    through: f32,
    frame: &EventFrame<'_>,
    out: &mut MessageWriter<'_, AnimEvent>,
) {
    let after = match scan {
        TrackScan::Arming => return,
        TrackScan::FromHead => f32::NEG_INFINITY,
        TrackScan::After(seek) => seek,
    };
    #[allow(clippy::float_cmp, reason = "a clip that did not move crossed nothing")]
    if clip.events.is_empty() || through == after {
        return;
    }
    let mut fire = |lo: f32, hi: f32| {
        for e in clip.events.iter() {
            if e.time > lo && e.time <= hi {
                out.write(AnimEvent {
                    entity,
                    ident: e.ident,
                    data: e.data,
                    anim_id: clip.anim_id,
                    pos: frame.point(e),
                });
            }
        }
    };
    if through >= after {
        fire(after, through);
    } else {
        fire(after, f32::INFINITY);
        fire(f32::NEG_INFINITY, through);
    }
}

type Driven<'a> = (
    Entity,
    &'a ModelAnimations,
    &'a AnimationPlayer,
    &'a AnimationTransitions,
    &'a GlobalTransform,
    Option<&'a RigPose>,
);

/// A unit's keys come from the clip it armed last, which is the newest variation while two
/// cross-fade.
pub(crate) fn fire_unit_events(
    units: Query<'_, '_, Driven<'_>>,
    globals: Query<'_, '_, &GlobalTransform>,
    mut last: Local<'_, TrackMemory>,
    mut out: MessageWriter<'_, AnimEvent>,
) {
    for (entity, anims, player, transitions, world, pose) in &units {
        let Some(node) = transitions.get_main_animation() else {
            continue;
        };
        let Some(clip) = anims.clips.iter().find(|c| c.node == node) else {
            continue;
        };
        let Some(cur) = player
            .animation(node)
            .map(bevy::animation::ActiveAnimation::seek_time)
        else {
            continue;
        };
        let scan = advance_track(&mut last, entity, node, cur);
        let frame = EventFrame {
            world,
            rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
        };
        scan_events(clip, entity, scan, cur, &frame, &mut out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arm_fires_nothing_and_the_next_frame_opens_the_head() {
        let (mut mem, e) = (TrackMemory::default(), Entity::from_raw_u32(1).expect("id"));
        let (a, b) = (AnimationNodeIndex::new(7), AnimationNodeIndex::new(11));
        assert_eq!(advance_track(&mut mem, e, a, 0.0), TrackScan::Arming);
        assert_eq!(advance_track(&mut mem, e, a, 0.016), TrackScan::FromHead);
        assert_eq!(
            advance_track(&mut mem, e, a, 0.032),
            TrackScan::After(0.016)
        );
        assert_eq!(advance_track(&mut mem, e, b, 0.5), TrackScan::Arming);
        assert_eq!(
            advance_track(&mut mem, e, b, 0.52),
            TrackScan::After(0.5),
            "an arm deep in its clip"
        );
        assert!(is_footstep_sound(b"$FSD") && !is_footstep_sound(b"$FL0"));
    }
}
