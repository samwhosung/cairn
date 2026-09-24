use bevy::prelude::*;

use crate::rig::AnimClip;

/// The movement flag word, the client's bits.
pub mod move_flags {
    pub const FORWARD: u32 = 0x1;
    pub const BACKWARD: u32 = 0x2;
    pub const STRAFE_LEFT: u32 = 0x4;
    pub const STRAFE_RIGHT: u32 = 0x8;
    pub const TURN_LEFT: u32 = 0x10;
    pub const TURN_RIGHT: u32 = 0x20;
    pub const WALK_MODE: u32 = 0x100;
    /// Rooted in place: an arc that ends rooted ends in mid-air, not in a landing.
    pub const ROOT: u32 = 0x1000;
    /// Airborne, for the whole arc.
    pub const FALLING: u32 = 0x2000;
    /// The arc has become a far fall: a jump that has dropped below its launch, or a step-off
    /// half a second on.
    pub const FALLING_FAR: u32 = 0x4000;
    pub const SWIMMING: u32 = 0x20_0000;
    pub const ANY_MOVE: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;
}

/// The `AnimationData.dbc` ids of the clips a walking body plays.
pub(crate) mod anim {
    pub const STAND: u16 = 0;
    pub const WALK: u16 = 4;
    pub const RUN: u16 = 5;
    pub const SHUFFLE_LEFT: u16 = 11;
    pub const SHUFFLE_RIGHT: u16 = 12;
    pub const WALK_BACKWARDS: u16 = 13;
    pub const JUMP_START: u16 = 37;
    pub const JUMP: u16 = 38;
    pub const JUMP_END: u16 = 39;
    pub const FALL: u16 = 40;
    pub const SWIM_IDLE: u16 = 41;
    pub const SWIM: u16 = 42;
    pub const SWIM_LEFT: u16 = 43;
    pub const SWIM_RIGHT: u16 = 44;
    pub const SWIM_BACKWARDS: u16 = 45;
    pub const FLY: u16 = 135;
    pub const SPRINT: u16 = 143;
    pub const JUMP_LAND_RUN: u16 = 187;
    pub const SIT_GROUND_DOWN: u16 = 96;
    pub const SIT_GROUND: u16 = 97;
    pub const SIT_GROUND_UP: u16 = 98;
    pub const SLEEP_DOWN: u16 = 99;
    pub const SLEEP: u16 = 100;
    pub const SLEEP_UP: u16 = 101;
    pub const KNEEL_START: u16 = 114;
    pub const KNEEL_LOOP: u16 = 115;
    pub const KNEEL_END: u16 = 116;
}

/// A unit's stand state, the client's unit field: standing, or a pose it holds in place.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StandState(pub u8);

impl StandState {
    pub const STAND: Self = Self(0);
    pub const SIT: Self = Self(1);
    pub const SLEEP: Self = Self(3);
    pub const KNEEL: Self = Self(8);

    fn stand_up_id(self) -> u16 {
        match self {
            Self::SIT => SIT_GROUND_UP,
            Self::SLEEP => SLEEP_UP,
            Self::KNEEL => KNEEL_END,
            _ => STAND,
        }
    }
}

use anim::{
    FALL, FLY, JUMP, JUMP_END, JUMP_LAND_RUN, JUMP_START, KNEEL_END, KNEEL_LOOP, KNEEL_START, RUN,
    SHUFFLE_LEFT, SHUFFLE_RIGHT, SIT_GROUND, SIT_GROUND_DOWN, SIT_GROUND_UP, SLEEP, SLEEP_DOWN,
    SLEEP_UP, SPRINT, STAND, SWIM, SWIM_BACKWARDS, SWIM_IDLE, SWIM_LEFT, SWIM_RIGHT, WALK,
    WALK_BACKWARDS,
};

/// A unit's movement this frame, as its animation reads it. A unit without one stands.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct UnitMotion {
    /// The speed it travels at, yd/s, already the backward speed when backing; for a swimmer the
    /// stroke's speed whatever its pitch.
    pub speed: f32,
    /// Vertical speed, yd/s, up positive: how fast an arc rises on its first frame tells a jump
    /// from a step-off.
    pub vertical_speed: f32,
    pub flags: u32,
    /// The pose it holds while it stands still.
    pub stand_state: StandState,
}

pub(crate) const DEFAULT_WALK_SPEED: f32 = 2.5;
const SPRINT_SPEED: f32 = 11.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Bracketed {
    Jump,
    Fall,
    Pose(StandState),
}

impl Bracketed {
    pub(crate) fn entry_id(self) -> u16 {
        match self {
            Self::Jump => JUMP_START,
            Self::Fall => FALL,
            Self::Pose(StandState::SIT) => SIT_GROUND_DOWN,
            Self::Pose(StandState::SLEEP) => SLEEP_DOWN,
            Self::Pose(StandState::KNEEL) => KNEEL_START,
            Self::Pose(_) => STAND,
        }
    }

    pub(crate) fn loop_id(self) -> u16 {
        match self {
            Self::Jump => JUMP,
            Self::Fall => FALL,
            Self::Pose(StandState::SIT) => SIT_GROUND,
            Self::Pose(StandState::SLEEP) => SLEEP,
            Self::Pose(StandState::KNEEL) => KNEEL_LOOP,
            Self::Pose(_) => STAND,
        }
    }

    pub(crate) fn stand_up_id(self) -> Option<u16> {
        match self {
            Self::Pose(pose) => Some(pose.stand_up_id()),
            Self::Jump | Self::Fall => None,
        }
    }

    pub(crate) fn airborne(self) -> bool {
        matches!(self, Self::Jump | Self::Fall)
    }
}

pub(crate) fn jump_land_pick(flags: u32) -> Option<u16> {
    use move_flags::{ANY_MOVE, BACKWARD, ROOT, SWIMMING, WALK_MODE};
    if flags & (SWIMMING | ROOT) != 0 {
        None
    } else if flags & ANY_MOVE == 0 {
        Some(JUMP_END)
    } else if flags & (BACKWARD | WALK_MODE) == 0 {
        Some(JUMP_LAND_RUN)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Mode {
    #[default]
    Gait,
    Entering(Bracketed),
    Looping(Bracketed),
    Land {
        id: u16,
        touchdown_flags: u32,
    },
    StandingUp {
        pose: StandState,
        clip: u16,
    },
}

/// The gait a unit plays, the wanted id first and the fallbacks after it.
pub(crate) fn gait_candidates(motion: &UnitMotion, walk_speed: f32) -> &'static [u16] {
    use move_flags::{
        ANY_MOVE, BACKWARD, FORWARD, STRAFE_LEFT, STRAFE_RIGHT, SWIMMING, TURN_LEFT, TURN_RIGHT,
    };
    let f = motion.flags;
    if f & SWIMMING != 0 {
        return if f & (TURN_LEFT | TURN_RIGHT) != 0 {
            &[SWIM_IDLE, STAND]
        } else if f & STRAFE_LEFT != 0 {
            &[SWIM_LEFT, SWIM, SWIM_IDLE, STAND]
        } else if f & STRAFE_RIGHT != 0 {
            &[SWIM_RIGHT, SWIM, SWIM_IDLE, STAND]
        } else if f & BACKWARD != 0 {
            &[SWIM_BACKWARDS, SWIM_IDLE, STAND]
        } else if f & FORWARD != 0 {
            &[SWIM, SWIM_IDLE, STAND]
        } else {
            &[SWIM_IDLE, STAND]
        };
    }
    if f & BACKWARD != 0 {
        return &[WALK_BACKWARDS, WALK, STAND];
    }
    if f & ANY_MOVE != 0 {
        let s = motion.speed;
        return if s >= SPRINT_SPEED {
            &[SPRINT, RUN, WALK, STAND]
        } else if s > 2.0 * walk_speed {
            &[RUN, WALK, STAND]
        } else {
            &[WALK, STAND]
        };
    }
    if f & TURN_LEFT != 0 {
        return &[SHUFFLE_LEFT, STAND];
    }
    if f & TURN_RIGHT != 0 {
        return &[SHUFFLE_RIGHT, STAND];
    }
    &[STAND]
}

pub(crate) fn current_bracket(motion: &UnitMotion, jump_arc: bool) -> Option<Bracketed> {
    let f = motion.flags;
    if f & move_flags::FALLING != 0 {
        if f & move_flags::FALLING_FAR != 0 {
            Some(Bracketed::Fall)
        } else if jump_arc {
            Some(Bracketed::Jump)
        } else {
            None
        }
    } else if f & move_flags::ANY_MOVE == 0
        && matches!(
            motion.stand_state,
            StandState::SIT | StandState::SLEEP | StandState::KNEEL
        )
    {
        Some(Bracketed::Pose(motion.stand_state))
    } else {
        None
    }
}

const SPEED_SCALED_IDS: &[u16] = &[
    WALK,
    RUN,
    SHUFFLE_LEFT,
    SHUFFLE_RIGHT,
    WALK_BACKWARDS,
    JUMP_START,
    JUMP,
    JUMP_END,
    SWIM,
    SWIM_LEFT,
    SWIM_RIGHT,
    SWIM_BACKWARDS,
    FLY,
    SPRINT,
    JUMP_LAND_RUN,
];

/// The rate `clip` plays at for a unit moving at `speed` drawn at `model_scale`, where the rate
/// follows the speed at all.
pub(crate) fn scaled_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> Option<f32> {
    let divisor = clip.move_speed * model_scale.abs();
    (divisor > 0.0 && SPEED_SCALED_IDS.contains(&clip.anim_id)).then(|| speed / divisor)
}

pub(crate) fn playback_rate(clip: &AnimClip, speed: f32, model_scale: f32) -> f32 {
    scaled_rate(clip, speed, model_scale).unwrap_or(1.0)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use bevy::animation::graph::AnimationNodeIndex;

    use super::move_flags::*;
    use super::*;

    fn moving(flags: u32, speed: f32) -> UnitMotion {
        UnitMotion {
            speed,
            flags,
            ..UnitMotion::default()
        }
    }

    fn clip(anim_id: u16, move_speed: f32) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(0),
            looping: true,
            duration: 1.0,
            move_speed,
            blend_time: 0.25,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
            events: std::sync::Arc::from([]),
        }
    }

    #[test]
    fn a_unit_at_rest_stands() {
        assert_eq!(gait_candidates(&UnitMotion::default(), 2.5), &[STAND]);
    }

    #[test]
    fn a_run_is_over_twice_the_walk_speed_and_a_sprint_from_eleven() {
        assert_eq!(gait_candidates(&moving(FORWARD, 4.9), 2.5)[0], 4);
        assert_eq!(gait_candidates(&moving(FORWARD, 5.0), 2.5)[0], 4);
        assert_eq!(gait_candidates(&moving(FORWARD, 5.1), 2.5)[0], 5);
        assert_eq!(gait_candidates(&moving(FORWARD, 7.0), 4.0)[0], 4);
        assert_eq!(
            gait_candidates(&moving(FORWARD, 11.0), 2.5),
            &[143, 5, 4, 0]
        );
    }

    #[test]
    fn backing_outranks_strafing_and_speed() {
        assert_eq!(
            gait_candidates(&moving(BACKWARD | STRAFE_LEFT, 9.0), 2.5),
            &[13, 4, 0]
        );
    }

    #[test]
    fn a_swimmer_turns_over_strafes_over_backs_over_strokes() {
        let swim = |flags| gait_candidates(&moving(SWIMMING | flags, 4.0), 2.5);
        assert_eq!(swim(BACKWARD), &[45, 41, 0]);
        assert_eq!(swim(FORWARD), &[42, 41, 0]);
        assert_eq!(swim(STRAFE_LEFT), &[43, 42, 41, 0]);
        assert_eq!(swim(STRAFE_RIGHT), &[44, 42, 41, 0]);
        assert_eq!(swim(FORWARD | STRAFE_LEFT), &[43, 42, 41, 0]);
        assert_eq!(swim(BACKWARD | STRAFE_RIGHT), &[44, 42, 41, 0]);
        assert_eq!(swim(FORWARD | BACKWARD), &[45, 41, 0]);
        assert_eq!(swim(FORWARD | TURN_LEFT), &[41, 0]);
        assert_eq!(swim(0), &[41, 0]);
    }

    #[test]
    fn turning_in_place_shuffles_and_turning_on_the_move_runs() {
        assert_eq!(gait_candidates(&moving(TURN_LEFT, 0.0), 2.5), &[11, 0]);
        assert_eq!(gait_candidates(&moving(TURN_RIGHT, 0.0), 2.5), &[12, 0]);
        assert_eq!(
            gait_candidates(&moving(FORWARD | TURN_LEFT, 3.0), 2.5),
            &[4, 0]
        );
    }

    #[test]
    fn the_air_splits_into_jump_fall_and_a_held_gait() {
        let arc = moving(FORWARD | FALLING, 7.0);
        assert_eq!(current_bracket(&arc, true), Some(Bracketed::Jump));
        assert_eq!(current_bracket(&arc, false), None);
        let far = moving(FORWARD | FALLING | FALLING_FAR, 7.0);
        assert_eq!(current_bracket(&far, true), Some(Bracketed::Fall));
        assert_eq!(current_bracket(&far, false), Some(Bracketed::Fall));
        assert_eq!(
            (Bracketed::Jump.entry_id(), Bracketed::Jump.loop_id()),
            (37, 38)
        );
        assert_eq!(
            (Bracketed::Fall.entry_id(), Bracketed::Fall.loop_id()),
            (40, 40)
        );
    }

    #[test]
    fn a_pose_is_held_standing_still_and_brackets_its_loop() {
        let sat = |flags| UnitMotion {
            flags,
            stand_state: StandState::SIT,
            ..UnitMotion::default()
        };
        let seated = Some(Bracketed::Pose(StandState::SIT));
        assert_eq!(current_bracket(&sat(0), false), seated);
        assert_eq!(current_bracket(&sat(TURN_LEFT), false), seated);
        assert_eq!(current_bracket(&sat(FORWARD), false), None);
        let chair = UnitMotion {
            stand_state: StandState(4),
            ..UnitMotion::default()
        };
        assert_eq!(current_bracket(&chair, false), None);
        for (state, ids) in [
            (StandState::SIT, (96, 97, Some(98))),
            (StandState::SLEEP, (99, 100, Some(101))),
            (StandState::KNEEL, (114, 115, Some(116))),
        ] {
            let p = Bracketed::Pose(state);
            assert_eq!((p.entry_id(), p.loop_id(), p.stand_up_id()), ids);
        }
        assert_eq!(Bracketed::Jump.stand_up_id(), None);
    }

    #[test]
    fn the_landing_is_picked_from_the_flags_at_touchdown() {
        assert_eq!(jump_land_pick(0), Some(39));
        assert_eq!(jump_land_pick(FORWARD), Some(187));
        assert_eq!(jump_land_pick(STRAFE_LEFT), Some(187));
        assert_eq!(jump_land_pick(FORWARD | STRAFE_RIGHT), Some(187));
        assert_eq!(jump_land_pick(BACKWARD), None);
        assert_eq!(jump_land_pick(BACKWARD | STRAFE_LEFT), None);
        assert_eq!(jump_land_pick(FORWARD | WALK_MODE), None);
        assert_eq!(jump_land_pick(SWIMMING), None);
        assert_eq!(jump_land_pick(FORWARD | SWIMMING), None);
        assert_eq!(jump_land_pick(ROOT | FORWARD), None);
    }

    #[test]
    fn locomotion_plays_at_the_speed_over_its_authored_speed() {
        assert!((playback_rate(&clip(13, 2.5), 4.5, 1.0) - 1.8).abs() < 1e-5);
        assert!((playback_rate(&clip(5, 7.0), 7.0, 1.0) - 1.0).abs() < 1e-5);
        assert!((playback_rate(&clip(4, 2.5), 4.0, 2.2) - 0.727_27).abs() < 1e-4);
        assert_eq!(playback_rate(&clip(0, 0.0), 9.0, 1.0), 1.0);
        assert_eq!(playback_rate(&clip(38, 0.0), 9.0, 1.0), 1.0);
        assert_eq!(playback_rate(&clip(60, 2.0), 9.0, 1.0), 1.0);
        assert_eq!(playback_rate(&clip(4, 2.5), 4.0, 0.0), 1.0);
        assert_eq!(playback_rate(&clip(13, -2.5), 4.5, 1.0), 1.0);
    }
}
