use std::time::Duration;

use bevy::animation::graph::AnimationNodeIndex;
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{ActiveAnimation, RepeatAnimation};
use bevy::prelude::*;

use super::motion::anim::{SHUFFLE_LEFT, SHUFFLE_RIGHT, STAND};
use super::motion::{
    Bracketed, DEFAULT_WALK_SPEED, Mode, UnitMotion, UnitShow, current_bracket, gait_candidates,
    is_wound, jump_land_pick, legs_take_up_locomotion, move_flags, moves_up_when_the_legs_move,
    playback_rate, plays_on_upper_body, scaled_rate,
};
use crate::rig::{AnimClip, AnimRng, ModelAnimations};
use wound::Wound;

mod wound;

const MIN_JUMP_LAUNCH_SPEED: f32 = 0.5;
const UPPER_BODY_OVER_GAIT: f32 = 8.0;
const UPPER_BODY_RELEASE_SECS: f32 = 0.150;

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
    upper_body_one_shot: Option<AnimationNodeIndex>,
    upper_body_fade: Option<UpperBodyFade>,
    flags_under_whole_body_one_shot: u32,
    wound: Option<Wound>,
}

#[derive(Clone, Copy)]
struct UpperBodyFade {
    fading_out: Option<AnimationNodeIndex>,
    secs_left: f32,
    total_secs: f32,
}

impl UpperBodyFade {
    fn outgoing_share(self) -> f32 {
        smoothstep(if self.total_secs > 0.0 {
            self.secs_left / self.total_secs
        } else {
            0.0
        })
    }

    fn nearer_its_start(self) -> bool {
        self.outgoing_share() > 0.5
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    (3.0 - 2.0 * t) * t * t
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

    fn game_holds_body(
        &mut self,
        mut show: Mut<'_, UnitShow>,
        f: &Frame<'_>,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        rng: &mut AnimRng,
    ) -> bool {
        if let Some(id) = show.play
            && show.play.take().is_some()
            && let Some(head) = resolved_clip(anims, id)
        {
            let (c, repeat) = roll_oneshot(anims, head, rng);
            if let Some(upper) = c.upper_node.filter(|_| plays_on_upper_body(id, &f.motion)) {
                self.play_on_upper_body(player, upper, repeat, c.blend_time);
                if matches!(self.mode, Mode::ShowPlayed(_)) {
                    self.mode = Mode::Gait;
                    self.armed_gait = None;
                }
            } else {
                if self.upper_body_one_shot.is_some() {
                    self.fade_upper_body(player, c.blend_time);
                }
                self.loop_window = None;
                play_clip(tr, player, c, repeat, 1.0);
                self.mode = Mode::ShowPlayed(id);
                self.flags_under_whole_body_one_shot = f.motion.flags;
                return true;
            }
        }
        if let Mode::ShowPlayed(id) = self.mode
            && !oneshot_finished(player, anims, id)
        {
            let flags_changed = f.motion.flags != self.flags_under_whole_body_one_shot;
            let legs_move_off = flags_changed && legs_take_up_locomotion(&f.motion, f.bracket);
            if !(legs_move_off && self.move_up_where_it_stands(tr, player, anims, id)) {
                return true;
            }
            self.mode = Mode::Gait;
            self.armed_gait = None;
        }
        if let Some(pose) = show.pose.filter(|&p| resolved_clip(anims, p).is_some()) {
            if self.mode != Mode::ShowPosed(pose) {
                self.hold(tr, player, anims, pose, rng);
                self.mode = Mode::ShowPosed(pose);
            }
            return true;
        }
        if matches!(self.mode, Mode::ShowPlayed(_) | Mode::ShowPosed(_)) {
            self.mode = Mode::Gait;
            self.armed_gait = None;
        }
        false
    }

    fn hold(
        &mut self,
        tr: &mut AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        pose: u16,
        rng: &mut AnimRng,
    ) {
        let Some(head) = resolved_clip(anims, pose) else {
            return;
        };
        if head.looping {
            self.play(tr, player, anims, pose, true, rng);
            return;
        }
        self.loop_window = None;
        let already_at_its_end = tr
            .get_main_animation()
            .and_then(|node| anims.clips.iter().find(|c| c.node == node))
            .is_some_and(|c| {
                c.anim_id == head.anim_id
                    && player
                        .animation(c.node)
                        .is_some_and(ActiveAnimation::is_finished)
            });
        if !already_at_its_end {
            play_clip(tr, player, head, RepeatAnimation::Never, 1.0);
            if let Some(active) = player.animation_mut(head.node) {
                active.seek_to(head.duration);
            }
        }
    }

    fn play_on_upper_body(
        &mut self,
        player: &mut AnimationPlayer,
        node: AnimationNodeIndex,
        repeat: RepeatAnimation,
        blend_secs: f32,
    ) {
        if self.upper_body_one_shot == Some(node) {
            player.start(node).set_repeat(repeat);
            return;
        }
        self.fade_upper_body(player, blend_secs);
        if let Some(fade) = &mut self.upper_body_fade
            && fade.fading_out == Some(node)
        {
            fade.fading_out = None;
        }
        player.start(node).set_repeat(repeat).set_weight(0.0);
        self.upper_body_one_shot = Some(node);
    }

    fn move_up_where_it_stands(
        &mut self,
        tr: &AnimationTransitions,
        player: &mut AnimationPlayer,
        anims: &ModelAnimations,
        id: u16,
    ) -> bool {
        if self.upper_body_one_shot.is_some() || !moves_up_when_the_legs_move(id) {
            return false;
        }
        let Some(node) = tr.get_main_animation() else {
            return false;
        };
        let Some((seek, speed)) = player
            .animation(node)
            .filter(|a| !a.is_finished())
            .map(|a| (a.seek_time(), a.speed()))
        else {
            return false;
        };
        let Some(upper) = anims
            .clips
            .iter()
            .find(|c| c.node == node)
            .and_then(|c| c.upper_node)
        else {
            return false;
        };
        if let Some(fade) = &mut self.upper_body_fade
            && fade.fading_out == Some(upper)
        {
            fade.fading_out = None;
        }
        player
            .start(upper)
            .set_repeat(RepeatAnimation::Never)
            .seek_to(seek)
            .set_speed(speed)
            .set_weight(UPPER_BODY_OVER_GAIT);
        self.upper_body_one_shot = Some(upper);
        true
    }

    /// The client lets a fade nearer its start than its end run on, and drops at once what a
    /// second one would have faded out.
    fn fade_upper_body(&mut self, player: &mut AnimationPlayer, secs: f32) {
        let fading_out = self.upper_body_one_shot.take();
        if self
            .upper_body_fade
            .is_some_and(UpperBodyFade::nearer_its_start)
        {
            if let Some(node) = fading_out {
                player.stop(node);
            }
            return;
        }
        if let Some(older) = self.upper_body_fade.take().and_then(|f| f.fading_out) {
            player.stop(older);
        }
        self.upper_body_fade = Some(UpperBodyFade {
            fading_out,
            secs_left: secs,
            total_secs: secs,
        });
    }

    fn release_played_out(&mut self, player: &mut AnimationPlayer) {
        if let Some(node) = self.upper_body_one_shot
            && player
                .animation(node)
                .is_none_or(ActiveAnimation::is_finished)
        {
            self.fade_upper_body(player, UPPER_BODY_RELEASE_SECS);
        }
    }

    fn advance_upper_body_fade(&mut self, player: &mut AnimationPlayer, dt: f32) {
        let Some(mut fade) = self.upper_body_fade else {
            return;
        };
        fade.secs_left = (fade.secs_left - dt).max(0.0);
        let (w, out_share) = (UPPER_BODY_OVER_GAIT, fade.outgoing_share());
        let playing = self.upper_body_one_shot;
        if let Some(active) = fade.fading_out.and_then(|n| player.animation_mut(n)) {
            active.set_weight(if playing.is_some() {
                w * out_share
            } else {
                w * out_share / (1.0 + w * (1.0 - out_share))
            });
        }
        if let Some(active) = playing.and_then(|n| player.animation_mut(n)) {
            active.set_weight(w * (1.0 - out_share));
        }
        if fade.secs_left > 0.0 {
            self.upper_body_fade = Some(fade);
        } else {
            if let Some(node) = fade.fading_out {
                player.stop(node);
            }
            self.upper_body_fade = None;
        }
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

    fn freeze_arc(
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
            self.freeze_arc(left, tr, player, anims);
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
            Mode::ShowPlayed(_) | Mode::ShowPosed(_) => {
                self.mode = Mode::Gait;
                self.armed_gait = None;
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
        Option<&'static mut UnitShow>,
        &'static Transform,
    ),
>;

pub(crate) fn drive_units(
    time: Res<'_, Time>,
    mut rng: ResMut<'_, AnimRng>,
    mut units: Driven<'_, '_>,
) {
    for (mut drv, anims, mut player, mut tr, motion, mut show, transform) in &mut units {
        let motion = motion.copied().unwrap_or_default();
        drv.ease_wound(&mut player);
        let before = (drv.mode, drv.armed_gait, drv.upper_body_one_shot);
        let told_wound = show
            .as_mut()
            .and_then(|s| s.play.take_if(|id| is_wound(*id)));
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
        let shown = show
            .is_some_and(|s| drv.game_holds_body(s, &frame, &mut tr, &mut player, anims, &mut rng));
        if !shown {
            drv.run(&frame, &mut tr, &mut player, &mut rng);
        }
        drv.sync_rate(&tr, &mut player, anims, motion.speed, frame.model_scale);
        drv.release_played_out(&mut player);
        drv.advance_upper_body_fade(&mut player, time.delta_secs());
        let upper_body_played =
            drv.upper_body_one_shot.is_some() && drv.upper_body_one_shot != before.2;
        let whole_body_played = (drv.mode, drv.armed_gait) != (before.0, before.1);
        drv.evict_wound(&mut player, upper_body_played, whole_body_played);
        if let Some(id) = told_wound {
            drv.lay_wound(&tr, &mut player, anims, &mut rng, (id, &motion));
        }
    }
}

#[cfg(test)]
mod install;
#[cfg(test)]
mod tests;
