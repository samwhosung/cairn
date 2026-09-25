use std::sync::Arc;

use bevy::animation::graph::{AnimationGraph, AnimationNodeIndex};
use bevy::prelude::*;
use model::PlayableAnim;

use super::bake::GlobalBone;
use super::source::PoseSource;

/// One event key on a clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipEvent {
    /// Seconds from the clip's start.
    pub time: f32,
    /// As it reads, `*b"$FSD"`.
    pub ident: [u8; 4],
    /// The payload as read: a `SoundEntries` id for `$SND`, `$DSL` and `$DSO`.
    pub data: u32,
    pub bone: u16,
    /// The key's point from its bone's pivot, Bevy axes: composed with the bone's live global.
    pub offset: Vec3,
    /// The key's point in model space, Bevy axes: where it fires on a model with no live pose.
    pub point: Vec3,
}

/// One sequence of a model, playable through its animation graph.
#[derive(Clone, Debug)]
pub struct AnimClip {
    /// The `AnimationData.dbc` id, 0 for Stand.
    pub anim_id: u16,
    /// The sequence's index in the file.
    pub seq_index: usize,
    pub node: AnimationNodeIndex,
    /// The same sequence on the lower spine's subtree alone, or the head's on a model with no
    /// spine, so it plays over what the legs play; `None` on a model with neither key bone.
    pub upper_node: Option<AnimationNodeIndex>,
    pub looping: bool,
    /// The sequence's length, seconds.
    pub duration: f32,
    /// The ground speed the sequence was animated at, yd/s. Negative for an authored backwards
    /// gait, which the client plays at a flat rate.
    pub move_speed: f32,
    /// Seconds the client cross-fades into this sequence over.
    pub blend_time: f32,
    /// The sequence's bounding box, Bevy axes.
    pub bounds_min: Vec3,
    pub bounds_max: Vec3,
    /// This variation's weight when the client rolls among the sequences sharing `anim_id`.
    pub frequency: u16,
    /// The range the client rolls a play count from each time it arms the sequence.
    pub replay: (u32, u32),
    /// Some bone channel moves; a clip that poses nothing still runs a clock.
    pub poses_bones: bool,
    /// The event keys, by time.
    pub events: Arc<[ClipEvent]>,
}

/// A model's sequences, one graph shared by every instance that plays them.
#[derive(Clone, Component)]
pub struct ModelAnimations {
    pub graph: Handle<AnimationGraph>,
    pub clips: Vec<AnimClip>,
    /// For each requested `AnimationData.dbc` id, the id this model plays instead. Empty when the
    /// model has no table.
    pub playable_animation_lookup: Vec<PlayableAnim>,
    /// For each `AnimationData.dbc` id, the model's first sequence slot with it, `0xffff` for
    /// none.
    pub animation_lookup: Vec<u16>,
    pub global_bones: Vec<GlobalBone>,
    /// Index into `clips` of the first sequence with the idle's id whose pose differs from rest;
    /// `None` when none does.
    pub moving_idle: Option<usize>,
    pub pose: Arc<PoseSource>,
}

/// A requested animation id resolved against one model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedAnim {
    pub id: u16,
    /// The lookup row's direction or variant code; carried, not applied.
    pub dir_flags: u16,
}

const ANIMATION_DATA_ROWS: usize = 208;

impl ModelAnimations {
    /// The first variation of `anim_id`, or `None` when the model has no such sequence.
    pub fn find(&self, anim_id: u16) -> Option<&AnimClip> {
        self.clips.iter().find(|c| c.anim_id == anim_id)
    }

    /// Whether the model authors a sequence for `anim_id` at all, clip or not.
    pub fn owns(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// What a model instance plays when it loads: Stand as the model's own lookup resolves it,
    /// else the first sequence.
    pub fn idle_clip(&self) -> Option<&AnimClip> {
        let idle_id = self
            .playable_animation_lookup
            .first()
            .map_or(0, |p| p.resolved_id);
        self.clips
            .iter()
            .find(|c| c.anim_id == idle_id)
            .or_else(|| self.clips.first())
    }

    /// The variation of `anim_id` a roll of `0..=0x7fff` picks: each variation in file order takes
    /// the roll if it is under its weight, else the roll drops by that weight; when the weights run
    /// out the first variation plays. `None` when the model has no sequence for `anim_id`.
    pub fn pick_variation(&self, anim_id: u16, roll: u16) -> Option<&AnimClip> {
        let mut roll = i32::from(roll);
        let mut head = None;
        for c in self.clips.iter().filter(|c| c.anim_id == anim_id) {
            head.get_or_insert(c);
            if roll < i32::from(c.frequency) {
                return Some(c);
            }
            roll -= i32::from(c.frequency);
        }
        head
    }

    /// The id this model plays for `requested`: its own lookup row when the table covers the id,
    /// the id itself when the model has no table, and past the table the `AnimationData.dbc`
    /// fallback chain (`fallback`, `None` for Stand or an unknown row) until the model has the id,
    /// Stand when the chain runs out or loops.
    pub fn resolve(&self, requested: u16, fallback: &dyn Fn(u16) -> Option<u16>) -> ResolvedAnim {
        const STAND: ResolvedAnim = ResolvedAnim {
            id: 0,
            dir_flags: 0,
        };
        if let Some(row) = self.playable_animation_lookup.get(requested as usize) {
            return ResolvedAnim {
                id: row.resolved_id,
                dir_flags: row.dir_flags,
            };
        }
        if self.playable_animation_lookup.is_empty() {
            return ResolvedAnim {
                id: requested,
                dir_flags: 0,
            };
        }
        let mut visited = [false; ANIMATION_DATA_ROWS];
        let mut id = requested;
        loop {
            if self.find(id).is_some() {
                return ResolvedAnim { id, dir_flags: 0 };
            }
            match visited.get_mut(id as usize) {
                Some(seen) if !*seen => *seen = true,
                _ => return STAND,
            }
            match fallback(id) {
                Some(next) => id = next,
                None => return STAND,
            }
        }
    }

    /// The clip `requested` resolves to on this model, if it has one.
    pub fn find_resolved(
        &self,
        requested: u16,
        fallback: &dyn Fn(u16) -> Option<u16>,
    ) -> Option<&AnimClip> {
        self.find(self.resolve(requested, fallback).id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(anim_id: u16) -> AnimClip {
        AnimClip {
            anim_id,
            seq_index: 0,
            node: AnimationNodeIndex::new(0),
            upper_node: None,
            looping: true,
            duration: 1.0,
            move_speed: 0.0,
            blend_time: 0.0,
            bounds_min: Vec3::ZERO,
            bounds_max: Vec3::ZERO,
            frequency: 0,
            replay: (0, 0),
            poses_bones: true,
            events: Arc::from([]),
        }
    }

    fn anims(ids: &[u16], table: Vec<PlayableAnim>) -> ModelAnimations {
        ModelAnimations {
            graph: Handle::default(),
            clips: ids.iter().map(|&id| clip(id)).collect(),
            playable_animation_lookup: table,
            animation_lookup: Vec::new(),
            global_bones: Vec::new(),
            moving_idle: None,
            pose: Arc::default(),
        }
    }

    fn row(resolved_id: u16, dir_flags: u16) -> PlayableAnim {
        PlayableAnim {
            resolved_id,
            dir_flags,
        }
    }

    fn none(_: u16) -> Option<u16> {
        None
    }

    #[test]
    fn the_lookup_table_answers_the_ids_it_covers() {
        let mut table = vec![row(0, 0); 19];
        table[18] = row(16, 0);
        table[6] = row(1, 3);
        let a = anims(&[0, 1, 16], table);
        assert_eq!(
            a.resolve(18, &none),
            ResolvedAnim {
                id: 16,
                dir_flags: 0
            }
        );
        assert_eq!(
            a.resolve(6, &none),
            ResolvedAnim {
                id: 1,
                dir_flags: 3
            }
        );
    }

    #[test]
    fn past_the_table_the_fallback_chain_walks_until_the_model_has_the_id() {
        let chain = |id: u16| match id {
            5 => Some(4),
            4 => Some(3),
            7 => Some(8),
            8 => Some(7),
            _ => None,
        };
        let a = anims(&[0, 3], vec![row(0, 0); 3]);
        assert_eq!(a.resolve(5, &chain).id, 3);
        assert_eq!(a.resolve(7, &chain).id, 0, "a loop ends at Stand");
        assert_eq!(a.resolve(99, &chain).id, 0);
        assert_eq!(a.resolve(250, &chain).id, 0, "past the table's rows");
        assert_eq!(anims(&[0], Vec::new()).resolve(42, &none).id, 42);
    }

    #[test]
    fn the_idle_is_stand_through_the_lookup_then_the_first_sequence() {
        let mut a = anims(&[145, 0, 157], Vec::new());
        for (i, c) in a.clips.iter_mut().enumerate() {
            c.seq_index = i;
        }
        assert_eq!(a.idle_clip().map(|c| c.seq_index), Some(1));
        let b = anims(&[159, 158], vec![row(158, 0)]);
        assert_eq!(b.idle_clip().map(|c| c.anim_id), Some(158));
        assert_eq!(
            anims(&[158, 159], Vec::new())
                .idle_clip()
                .map(|c| c.anim_id),
            Some(158)
        );
    }

    #[test]
    fn a_variation_takes_the_roll_under_its_weight() {
        let mut a = anims(&[16, 16, 5], Vec::new());
        a.clips[0].frequency = 0x6000;
        a.clips[1].frequency = 0x1fff;
        let pick = |a: &ModelAnimations, id, roll| {
            let c = a.pick_variation(id, roll).expect("has the id");
            a.clips.iter().position(|x| std::ptr::eq(x, c))
        };
        assert_eq!(pick(&a, 16, 0x0000), Some(0));
        assert_eq!(pick(&a, 16, 0x5fff), Some(0));
        assert_eq!(pick(&a, 16, 0x6000), Some(1));
        assert_eq!(pick(&a, 16, 0x7ffe), Some(1));
        assert_eq!(pick(&a, 16, 0x7fff), Some(0), "the weights ran out");
        assert_eq!(pick(&a, 5, 0x7fff), Some(2));
        assert!(a.pick_variation(99, 0).is_none());
    }
}
