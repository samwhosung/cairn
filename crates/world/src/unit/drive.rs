use std::time::Duration;

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{ActiveAnimation, RepeatAnimation};
use bevy::prelude::*;

use super::motion::anim::{SHUFFLE_LEFT, SHUFFLE_RIGHT, STAND};
use super::motion::{
    Bracketed, DEFAULT_WALK_SPEED, Mode, UnitMotion, current_bracket, gait_candidates,
    jump_land_pick, move_flags, playback_rate, scaled_rate,
};
use crate::rig::{AnimClip, AnimRng, ModelAnimations};

const MIN_JUMP_LAUNCH_SPEED: f32 = 0.5;

/// A unit's animation state across frames.
#[derive(Component, Default)]
pub struct UnitDriver {
    mode: Mode,
    armed_gait: Option<u16>,
    loop_window: Option<LoopWindow>,
    jump_arc: bool,
    last_vertical_speed: f32,
    was_falling: bool,
    frozen_airborne: Option<AnimationNodeIndex>,
}

/// A looping clip armed on the base track, and the passes it plays before it is rolled again.
#[derive(Clone, Copy)]
struct LoopWindow {
    node: AnimationNodeIndex,
    passes: u32,
}

impl LoopWindow {
    fn played_out(self, player: &AnimationPlayer) -> bool {
        player
            .animation(self.node)
            .is_some_and(|a| a.completions() >= self.passes)
    }

    fn still_playing(self, player: &AnimationPlayer) -> bool {
        player
            .animation(self.node)
            .is_some_and(|a| a.completions() < self.passes)
    }
}

fn resolved_clip(anims: &ModelAnimations, id: u16) -> Option<&AnimClip> {
    anims.find_resolved(id, &|_| None)
}

fn play_clip(
    tr: &mut AnimationTransitions,
    player: &mut AnimationPlayer,
    clip: &AnimClip,
    repeat: RepeatAnimation,
    rate: f32,
) {
    let blend = Duration::from_secs_f32(clip.blend_time.max(0.0));
    let active = tr.play(player, clip.node, blend);
    active.set_repeat(repeat);
    active.set_speed(rate);
}

/// An arm's two rolls in the client's order: the variation, then the passes it plays.
fn roll_loop<'a>(
    anims: &'a ModelAnimations,
    head: &'a AnimClip,
    rng: &mut AnimRng,
) -> (&'a AnimClip, u32) {
    let c = anims
        .pick_variation(head.anim_id, rng.draw())
        .unwrap_or(head);
    (c, rng.replay_count(c.replay))
}

fn roll_oneshot<'a>(
    anims: &'a ModelAnimations,
    head: &'a AnimClip,
    rng: &mut AnimRng,
) -> (&'a AnimClip, RepeatAnimation) {
    let (c, passes) = roll_loop(anims, head, rng);
    let repeat = if passes > 1 {
        RepeatAnimation::Count(passes)
    } else {
        RepeatAnimation::Never
    };
    (c, repeat)
}

struct Frame<'a> {
    anims: &'a ModelAnimations,
    motion: UnitMotion,
    bracket: Option<Bracketed>,
    /// A step-off in the air: its gait keeps rolling as it left the ground.
    gait_held_aloft: bool,
    model_scale: f32,
}

impl UnitDriver {
    fn play(
        &mut self,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        id: u16,
        looping: bool,
        rng: &mut AnimRng,
    ) {
        let Some(head) = resolved_clip(anims, id) else {
            return;
        };
        let (c, repeat) = if looping {
            let (c, passes) = roll_loop(anims, head, rng);
            self.loop_window = Some(LoopWindow {
                node: c.node,
                passes,
            });
            (c, RepeatAnimation::Forever)
        } else {
            self.loop_window = None;
            roll_oneshot(anims, head, rng)
        };
        play_clip(tr, player, c, repeat, 1.0);
    }

    fn enter_bracket(
        &mut self,
        bracket: Bracketed,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        rng: &mut AnimRng,
    ) -> Mode {
        if bracket == Bracketed::Fall {
            self.play(tr, player, anims, bracket.loop_id(), true, rng);
            Mode::Looping(bracket)
        } else {
            self.play(tr, player, anims, bracket.entry_id(), false, rng);
            Mode::Entering(bracket)
        }
    }

    fn freeze_arc_for_landing(
        &mut self,
        arc: Bracketed,
        tr: &AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
    ) {
        if let Some(node) = tr.get_main_animation()
            && plays_airborne_clip(anims, arc, node)
            && let Some(active) = player.animation_mut(node)
        {
            active.set_speed(0.0);
            self.frozen_airborne = Some(node);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn leave_bracket(
        &mut self,
        left: Bracketed,
        next: Option<Bracketed>,
        flags: u32,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        rng: &mut AnimRng,
    ) -> Mode {
        if left.airborne() {
            self.freeze_arc_for_landing(left, tr, player, anims);
        }
        if let Some(next) = next {
            return self.enter_bracket(next, tr, player, anims, rng);
        }
        let (Bracketed::Pose(pose), Some(clip)) = (left, left.stand_up_id()) else {
            return match jump_land_pick(flags) {
                Some(id) => {
                    self.play(tr, player, anims, id, false, rng);
                    Mode::Land {
                        id,
                        touchdown_flags: flags,
                    }
                }
                None => Mode::Gait,
            };
        };
        if flags & move_flags::ANY_MOVE != 0 {
            return Mode::Gait;
        }
        self.play(tr, player, anims, clip, false, rng);
        Mode::StandingUp { pose, clip }
    }

    fn run(
        &mut self,
        f: &Frame<'_>,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        rng: &mut AnimRng,
    ) {
        let (anims, mv, bracket) = (f.anims, f.motion, f.bracket);
        match self.mode {
            Mode::Entering(entered) => {
                let swimmer_finishes_the_kick = entered == Bracketed::Jump
                    && bracket.is_none()
                    && mv.flags & move_flags::SWIMMING != 0
                    && !oneshot_finished(player, anims, entered.entry_id());
                if swimmer_finishes_the_kick {
                    return;
                }
                if bracket != Some(entered) {
                    self.mode =
                        self.leave_bracket(entered, bracket, mv.flags, tr, player, anims, rng);
                } else if oneshot_finished(player, anims, entered.entry_id()) {
                    self.play(tr, player, anims, entered.loop_id(), true, rng);
                    self.mode = Mode::Looping(entered);
                }
            }
            Mode::Looping(held) => {
                if bracket != Some(held) {
                    self.mode = self.leave_bracket(held, bracket, mv.flags, tr, player, anims, rng);
                }
            }
            Mode::Land {
                id,
                touchdown_flags,
            } => {
                if let Some(next) = bracket {
                    self.mode = self.enter_bracket(next, tr, player, anims, rng);
                } else if mv.flags != touchdown_flags || oneshot_finished(player, anims, id) {
                    self.mode = Mode::Gait;
                    self.armed_gait = None;
                }
            }
            Mode::StandingUp { clip, .. } => {
                if let Some(next) = bracket {
                    self.mode = self.enter_bracket(next, tr, player, anims, rng);
                } else if mv.flags & move_flags::ANY_MOVE != 0
                    || oneshot_finished(player, anims, clip)
                {
                    self.mode = Mode::Gait;
                    self.armed_gait = None;
                }
            }
            Mode::Gait => {
                if let Some(next) = bracket {
                    self.mode = self.enter_bracket(next, tr, player, anims, rng);
                    self.armed_gait = None;
                } else if !(f.gait_held_aloft && self.armed_gait.is_some()) {
                    self.run_gait(f, tr, player, rng);
                }
            }
        }
    }

    /// A turn shuffle is held to its own window against Stand: the turn ending does not stop
    /// it, its window does.
    fn run_gait(
        &mut self,
        f: &Frame<'_>,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        rng: &mut AnimRng,
    ) {
        let cands = gait_candidates(&f.motion, DEFAULT_WALK_SPEED);
        let shuffling = self.loop_window.is_some_and(|w| w.still_playing(player));
        let target = match self.armed_gait {
            Some(g @ (SHUFFLE_LEFT | SHUFFLE_RIGHT)) if cands[0] == STAND && shuffling => g,
            _ => cands[0],
        };
        if self.armed_gait != Some(target) {
            if let Some(head) = cands.iter().find_map(|&id| resolved_clip(f.anims, id)) {
                let (c, passes) = roll_loop(f.anims, head, rng);
                self.loop_window = Some(LoopWindow {
                    node: c.node,
                    passes,
                });
                let rate = playback_rate(c, f.motion.speed, f.model_scale);
                play_clip(tr, player, c, RepeatAnimation::Forever, rate);
            }
            self.armed_gait = Some(target);
        }
    }

    /// In the gait a played-out loop picks the gait afresh; aloft it arms the same id again with
    /// a fresh roll.
    fn advance_window(
        &mut self,
        anims: &ModelAnimations,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        rng: &mut AnimRng,
    ) {
        let Some(window) = self.loop_window else {
            return;
        };
        if tr.get_main_animation() != Some(window.node) || !window.played_out(player) {
            return;
        }
        if self.mode == Mode::Gait {
            self.armed_gait = None;
            return;
        }
        let head = anims
            .clips
            .iter()
            .find(|c| c.node == window.node)
            .and_then(|armed| resolved_clip(anims, armed.anim_id));
        if let Some(head) = head {
            let rate = player
                .animation(window.node)
                .map_or(1.0, ActiveAnimation::speed);
            let (c, passes) = roll_loop(anims, head, rng);
            play_clip(tr, player, c, RepeatAnimation::Forever, rate);
            self.loop_window = Some(LoopWindow {
                node: c.node,
                passes,
            });
        }
    }

    fn sync_rate(
        &mut self,
        tr: &AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        speed: f32,
        model_scale: f32,
    ) {
        let Some(node) = tr.get_main_animation() else {
            return;
        };
        if self.frozen_airborne == Some(node) {
            return;
        }
        self.frozen_airborne = None;
        if let Some(rate) = anims
            .clips
            .iter()
            .find(|c| c.node == node)
            .and_then(|c| scaled_rate(c, speed, model_scale))
            && let Some(active) = player.animation_mut(node)
        {
            active.set_speed(rate);
        }
    }
}

/// Whether none of the one-shot's variations still plays.
fn oneshot_finished(player: &AnimationPlayer, anims: &ModelAnimations, id: u16) -> bool {
    resolved_clip(anims, id).is_none_or(|head| {
        anims
            .clips
            .iter()
            .filter(|c| c.anim_id == head.anim_id)
            .all(|c| {
                player
                    .animation(c.node)
                    .is_none_or(ActiveAnimation::is_finished)
            })
    })
}

fn plays_airborne_clip(anims: &ModelAnimations, air: Bracketed, node: AnimationNodeIndex) -> bool {
    let Some(cur) = anims.clips.iter().find(|c| c.node == node) else {
        return false;
    };
    [air.entry_id(), air.loop_id()]
        .into_iter()
        .filter_map(|id| resolved_clip(anims, id))
        .any(|head| head.anim_id == cur.anim_id)
}

type Driven<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut UnitDriver,
        &'static ModelAnimations,
        &'static mut AnimationPlayer,
        &'static mut AnimationTransitions,
        Option<&'static UnitMotion>,
        &'static Transform,
    ),
>;

pub(crate) fn drive_units(mut rng: ResMut<'_, AnimRng>, mut units: Driven<'_, '_>) {
    for (mut drv, anims, mut player, mut tr, motion, transform) in &mut units {
        let motion = motion.copied().unwrap_or_default();
        let falling = motion.flags & move_flags::FALLING != 0;
        let was_falling = std::mem::replace(&mut drv.was_falling, falling);
        let prev_vertical = std::mem::replace(&mut drv.last_vertical_speed, motion.vertical_speed);
        let launched =
            motion.vertical_speed > MIN_JUMP_LAUNCH_SPEED && prev_vertical <= MIN_JUMP_LAUNCH_SPEED;
        if falling && (!was_falling || launched) {
            drv.jump_arc = motion.vertical_speed > MIN_JUMP_LAUNCH_SPEED;
        }
        let frame = Frame {
            anims,
            motion,
            bracket: current_bracket(&motion, drv.jump_arc),
            gait_held_aloft: falling
                && (motion.flags & move_flags::FALLING_FAR != 0 || motion.vertical_speed != 0.0),
            model_scale: transform.scale.x,
        };
        drv.advance_window(anims, &mut tr, &mut player, &mut rng);
        drv.run(&frame, &mut tr, &mut player, &mut rng);
        drv.sync_rate(&tr, &mut player, anims, motion.speed, frame.model_scale);
    }
}

#[cfg(test)]
mod tests;
