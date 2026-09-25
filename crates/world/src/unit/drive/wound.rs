use bevy::animation::RepeatAnimation;
use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;

use super::super::motion::anim::STAND;
use super::super::motion::{StandState, UnitMotion, wound_takes_whole_body};
use super::{UPPER_BODY_OVER_GAIT, UnitDriver, resolved_clip, smoothstep};
use crate::rig::{AnimRng, ModelAnimations};

/// The client's wound starts at this share of the pose and eases out over its clip, never
/// replacing it.
const WOUND_PEAK_SHARE: f32 = 0.75;

#[derive(Clone, Copy)]
pub(super) struct Wound {
    node: AnimationNodeIndex,
    span: f32,
    upper_body_only: bool,
}

fn wound_share(fraction_left: f32) -> f32 {
    smoothstep(fraction_left) * WOUND_PEAK_SHARE
}

fn weight_for_share(share: f32, the_rest: f32) -> f32 {
    the_rest * share / (1.0 - share)
}

impl UnitDriver {
    fn weight_under_wound(&self, upper_body_only: bool) -> f32 {
        if upper_body_only && self.upper_body_one_shot.is_some() {
            1.0 + UPPER_BODY_OVER_GAIT
        } else {
            1.0
        }
    }

    pub(super) fn ease_wound(&mut self, player: &mut AnimationPlayer) {
        let Some(wound) = self.wound else {
            return;
        };
        let the_rest = self.weight_under_wound(wound.upper_body_only);
        match player.animation_mut(wound.node) {
            Some(active) if !active.is_finished() => {
                let fraction_left = 1.0 - active.seek_time() / wound.span;
                active.set_weight(weight_for_share(wound_share(fraction_left), the_rest));
            }
            _ => {
                player.stop(wound.node);
                self.wound = None;
            }
        }
    }

    /// As in the client, the next play on the wound's own track takes its slot: a new one-shot
    /// above the spine, or on the base a new mode or gait.
    pub(super) fn evict_wound(
        &mut self,
        player: &mut AnimationPlayer,
        upper_body_played: bool,
        whole_body_played: bool,
    ) {
        if let Some(wound) = self.wound
            && (if wound.upper_body_only {
                upper_body_played
            } else {
                whole_body_played
            })
        {
            player.stop(wound.node);
            self.wound = None;
        }
    }

    pub(super) fn lay_wound(
        &mut self,
        tr: &AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        rng: &mut AnimRng,
        (id, motion): (u16, &UnitMotion),
    ) {
        if motion.stand_state == StandState::DEAD {
            return;
        }
        let base = tr
            .get_main_animation()
            .and_then(|node| anims.clips.iter().find(|c| c.node == node))
            .map_or(STAND, |c| c.anim_id);
        let Some(clip) = resolved_clip(anims, id)
            .and_then(|head| anims.pick_variation(head.anim_id, rng.draw()))
            .filter(|c| c.duration > 0.0)
        else {
            return;
        };
        let upper = clip
            .upper_node
            .filter(|_| !wound_takes_whole_body(id, base, motion.flags));
        let (node, upper_body_only) = upper.map_or((clip.node, false), |n| (n, true));
        if let Some(earlier) = self.wound.take() {
            player.stop(earlier.node);
        }
        if player.animation(node).is_some() {
            return;
        }
        let the_rest = self.weight_under_wound(upper_body_only);
        player
            .start(node)
            .set_repeat(RepeatAnimation::Never)
            .set_weight(weight_for_share(wound_share(1.0), the_rest));
        self.wound = Some(Wound {
            node,
            span: clip.duration,
            upper_body_only,
        });
    }
}
