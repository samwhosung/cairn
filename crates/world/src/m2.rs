use std::io;
use std::sync::Arc;

use bevy::animation::graph::AnimationGraph;
use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::camera::primitives::Aabb;
use bevy::image::Image;
use bevy::math::{Mat4, Vec3};
use bevy::prelude::Handle;
use bevy::reflect::TypePath;
use model::{
    M2Bounds, M2Light, ParticleEmitterDef, RibbonEmitterDef, Skeleton, m2_owner_reach,
    parse_m2_animation_lookup, parse_m2_animation_summary, parse_m2_animations,
    parse_m2_attachments, parse_m2_bounds, parse_m2_global_sequence_bones, parse_m2_lights,
    parse_m2_particle_emitters, parse_m2_playable_animation_lookup, parse_m2_render_submeshes,
    parse_m2_ribbon_emitters, parse_m2_skeleton,
};

use crate::coords::wow_to_bevy;
use crate::model::ModelSubmesh;
use crate::rig::{
    AnimClip, ClipEvent, ModelAnimations, ModelAttachment, ModelSkeleton, PoseSource,
    build_animation_clip, build_attachments, build_global_bones, build_skeleton, skeleton_pivots,
};
use crate::source::{Repeat, m2_url, texture_url};

/// An M2 as the world draws it: its render batches in skin order, its authored bounds and its
/// lights, and the skeleton and sequences it animates by.
#[derive(Asset, TypePath)]
pub struct M2Model {
    pub submeshes: Vec<ModelSubmesh>,
    /// `None` when the header's bounds do not read.
    pub bounds: Option<M2Bounds>,
    pub lights: Vec<M2Light>,
    /// Empty for a boneless model.
    pub skeleton: ModelSkeleton,
    pub inverse_bindposes: Arc<[Mat4]>,
    pub attachments: Vec<ModelAttachment>,
    /// `None` when nothing in the model moves with a sequence.
    pub animations: Option<ModelAnimations>,
    pub has_emitters: bool,
    pub(crate) emitters: Vec<ModelEmitter>,
    pub(crate) ribbons: Vec<ModelRibbon>,
}

#[derive(Clone)]
pub(crate) struct ModelEmitter {
    pub def: ParticleEmitterDef,
    pub texture: Option<Handle<Image>>,
    /// The emitter bone's pivot as the file gives it: model space, WoW axes.
    pub bone_pivot: [f32; 3],
    pub recursion: Option<Handle<M2Model>>,
    pub geometry: Option<Handle<M2Model>>,
    pub owner_reach: f32,
    pub idle_seq_index: usize,
}

#[derive(Clone)]
pub(crate) struct ModelRibbon {
    pub def: RibbonEmitterDef,
    pub texture: Option<Handle<Image>>,
    pub bone_pivot: [f32; 3],
    pub owner_reach: f32,
}

impl M2Model {
    /// The header's bounding box, model space in Bevy axes; `None` for a model that authors none
    /// or one with no extent.
    pub fn animated_bound(&self) -> Option<Aabb> {
        let b = self.bounds.as_ref()?;
        let (a, c) = (wow_to_bevy(b.bbox_min), wow_to_bevy(b.bbox_max));
        let (lo, hi) = (a.min(c), a.max(c));
        hi.cmpgt(lo).any().then(|| Aabb::from_min_max(lo, hi))
    }

    /// The bounding sphere a placement fades by: the header radius times the placement's scale,
    /// about the header box's centre (model space, Bevy axes). No bounds never fades.
    #[allow(
        clippy::manual_midpoint,
        reason = "the client's own sum-then-halve rounding"
    )]
    pub fn fade_sphere(&self, scale: f32) -> (f32, Vec3) {
        match &self.bounds {
            Some(b) => {
                let c = [
                    (b.bbox_min[0] + b.bbox_max[0]) * 0.5,
                    (b.bbox_min[1] + b.bbox_max[1]) * 0.5,
                    (b.bbox_min[2] + b.bbox_max[2]) * 0.5,
                ];
                (b.sphere_radius * scale, wow_to_bevy(c))
            }
            None => (f32::INFINITY, Vec3::ZERO),
        }
    }
}

#[derive(Default, TypePath)]
pub(crate) struct M2Loader;

impl AssetLoader for M2Loader {
    type Asset = M2Model;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<M2Model, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let subs = parse_m2_render_submeshes(&bytes, "", &[]).map_err(io::Error::other)?;
        let owner_reach = m2_owner_reach(&subs);
        let submeshes: Vec<ModelSubmesh> = subs
            .into_iter()
            .map(|sub| ModelSubmesh::load(ctx, sub))
            .collect();
        let raw_skeleton = parse_m2_skeleton(&bytes).unwrap_or_default();
        let (skeleton, inverse_bindposes) = build_skeleton(&raw_skeleton);
        let pivots = skeleton_pivots(&raw_skeleton);
        let attachments =
            build_attachments(&parse_m2_attachments(&bytes).unwrap_or_default(), &pivots);
        let has_emitters = parse_m2_animation_summary(&bytes)
            .is_ok_and(|s| s.particle_emitter_count > 0 || s.ribbon_emitter_count > 0);
        let animations = animations(ctx, &bytes, &skeleton, &pivots, &submeshes, has_emitters);
        let idle_seq = animations
            .as_ref()
            .and_then(ModelAnimations::idle_clip)
            .map_or(0, |c| c.seq_index);
        let (emitters, ribbons) = load_effects(ctx, &bytes, &raw_skeleton, owner_reach, idle_seq);
        Ok(M2Model {
            submeshes,
            bounds: parse_m2_bounds(&bytes).ok(),
            lights: parse_m2_lights(&bytes),
            skeleton,
            inverse_bindposes: inverse_bindposes.into(),
            attachments,
            animations,
            has_emitters,
            emitters,
            ribbons,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["m2"]
    }
}

fn load_effects(
    ctx: &mut LoadContext<'_>,
    bytes: &[u8],
    skeleton: &Skeleton,
    owner_reach: f32,
    idle_seq: usize,
) -> (Vec<ModelEmitter>, Vec<ModelRibbon>) {
    let pivot = |bone: u16| {
        skeleton
            .bones
            .get(bone as usize)
            .map_or([0.0; 3], |b| b.pivot)
    };
    let mut texture = |t: &Option<String>| {
        t.as_deref()
            .map(|t| ctx.load::<Image>(texture_url(t, Repeat::BOTH)))
    };
    let emitter_defs = parse_m2_particle_emitters(bytes);
    let ribbon_defs = parse_m2_ribbon_emitters(bytes);
    let emitter_textures: Vec<_> = emitter_defs.iter().map(|d| texture(&d.texture)).collect();
    let ribbons = ribbon_defs
        .into_iter()
        .map(|def| ModelRibbon {
            texture: texture(&def.texture),
            bone_pivot: pivot(def.bone),
            owner_reach,
            def,
        })
        .collect();
    let emitters = emitter_defs
        .into_iter()
        .zip(emitter_textures)
        .map(|(def, texture)| ModelEmitter {
            recursion: def
                .recursion_model
                .as_deref()
                .map(|p| ctx.load::<M2Model>(m2_url(p))),
            geometry: def
                .geometry_model
                .as_deref()
                .map(|p| ctx.load::<M2Model>(m2_url(p))),
            texture,
            bone_pivot: pivot(def.bone),
            owner_reach,
            idle_seq_index: idle_seq,
            def,
        })
        .collect();
    (emitters, ribbons)
}

fn animations(
    ctx: &mut LoadContext<'_>,
    bytes: &[u8],
    skeleton: &ModelSkeleton,
    pivots: &[Vec3],
    submeshes: &[ModelSubmesh],
    has_emitters: bool,
) -> Option<ModelAnimations> {
    let mut graph = AnimationGraph::new();
    let root = graph.root;
    let lower_body = lower_body_masks(skeleton);
    let mut pose = PoseSource {
        bone_masks: lower_body
            .clone()
            .unwrap_or_else(|| vec![0; skeleton.joints.len()]),
        ..PoseSource::default()
    };
    let playable_animation_lookup = parse_m2_playable_animation_lookup(bytes).unwrap_or_default();
    let idle_id = playable_animation_lookup
        .first()
        .map_or(0, |p| p.resolved_id);
    let mut moving_idle = None;
    let mut clips = Vec::new();
    for (i, anim) in parse_m2_animations(bytes).iter().enumerate() {
        if moving_idle.is_none() && anim.anim_id == idle_id && !anim.is_rest_pose() {
            moving_idle = Some(clips.len());
        }
        let (clip, pose_clip, poses_bones) = build_animation_clip(anim, skeleton);
        let pose_idx = pose.clips.len() as u32;
        pose.clips.push(pose_clip);
        let clip = ctx.add_labeled_asset(format!("clip{i}"), clip);
        let node = graph.add_clip(clip.clone(), 1.0, root);
        pose.set_node(node, pose_idx, 0);
        let upper_node = lower_body.is_some().then(|| {
            let upper = graph.add_clip_with_mask(clip, LOWER_BODY, 1.0, root);
            pose.set_node(upper, pose_idx, LOWER_BODY);
            upper
        });
        let (lo, hi) = (wow_to_bevy(anim.bounds_min), wow_to_bevy(anim.bounds_max));
        clips.push(AnimClip {
            anim_id: anim.anim_id,
            seq_index: anim.seq_index,
            node,
            upper_node,
            looping: anim.looping,
            duration: anim.duration,
            move_speed: anim.move_speed,
            blend_time: anim.blend_time,
            bounds_min: lo.min(hi),
            bounds_max: lo.max(hi),
            frequency: anim.frequency,
            replay: (anim.min_replay, anim.max_replay),
            poses_bones,
            events: anim
                .events
                .iter()
                .map(|e| ClipEvent {
                    time: e.time,
                    ident: e.ident,
                    data: e.data,
                    bone: e.bone,
                    offset: wow_to_bevy(e.position)
                        - pivots
                            .get(usize::from(e.bone))
                            .copied()
                            .unwrap_or(Vec3::ZERO),
                    point: wow_to_bevy(e.position),
                })
                .collect(),
        });
    }
    let global_bones = build_global_bones(&parse_m2_global_sequence_bones(bytes), skeleton);
    let samples_sequence = has_emitters
        || submeshes.iter().any(|s| {
            let g = &s.geometry;
            g.alpha_anim.is_some() || g.uv_anim.is_some() || g.rgb_anim.is_some()
        });
    let animates = clips.iter().any(|c| c.poses_bones)
        || !global_bones.is_empty()
        || (samples_sequence && !clips.is_empty());
    animates.then(|| ModelAnimations {
        graph: ctx.add_labeled_asset("anim_graph".to_owned(), graph),
        clips,
        playable_animation_lookup,
        animation_lookup: parse_m2_animation_lookup(bytes).unwrap_or_default(),
        global_bones,
        moving_idle,
        pose: Arc::new(pose),
    })
}

/// The mask group of the bones a one-shot on the upper body leaves to the gait.
const LOWER_BODY: u64 = 1;

/// Each bone's mask groups: [`LOWER_BODY`] for every bone outside the subtree of the lower spine,
/// or of the head on a model with no spine. `None` for a model with neither, which has no upper
/// body to play apart. A bone whose parent does not precede it is a root, as the pose composes it.
fn lower_body_masks(skeleton: &ModelSkeleton) -> Option<Vec<u64>> {
    let top = usize::from(skeleton.spine_bone.or(skeleton.head_bone)?);
    let mut upper = vec![false; skeleton.joints.len()];
    for (i, joint) in skeleton.joints.iter().enumerate() {
        let parent = usize::try_from(joint.parent).ok().filter(|&p| p < i);
        upper[i] = i == top || parent.is_some_and(|p| upper[p]);
    }
    Some(
        upper
            .into_iter()
            .map(|up| if up { 0 } else { LOWER_BODY })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::ModelJoint;

    fn skeleton(parents: &[i16], spine_bone: Option<u16>, head_bone: Option<u16>) -> ModelSkeleton {
        ModelSkeleton {
            joints: parents
                .iter()
                .map(|&parent| ModelJoint {
                    parent,
                    local_translation: Vec3::ZERO,
                    billboard: None,
                    parent_arm: None,
                })
                .collect(),
            spine_bone,
            head_bone,
        }
    }

    #[test]
    fn the_legs_and_the_pelvis_are_the_lower_body_and_the_spine_up_is_not() {
        const L: u64 = LOWER_BODY;
        // 0 pelvis, 1 and 2 thighs, 3 a shin, 4 the lower spine, 5 the chest, 6 an arm, 7 the head.
        let body = skeleton(&[-1, 0, 0, 1, 0, 4, 5, 5], Some(4), Some(7));
        assert_eq!(lower_body_masks(&body), Some(vec![L, L, L, L, 0, 0, 0, 0]));
        let headed = skeleton(&[-1, 0, 0, 1, 0, 4, 5, 5], None, Some(7));
        assert_eq!(
            lower_body_masks(&headed),
            Some(vec![L, L, L, L, L, L, L, 0]),
            "the head's subtree when there is no spine"
        );
        assert_eq!(lower_body_masks(&skeleton(&[-1, 0], None, None)), None);
        let looped = skeleton(&[-1, 2, 1], Some(0), None);
        assert_eq!(
            lower_body_masks(&looped),
            Some(vec![0, L, L]),
            "a parent that follows its bone makes the bone a root, so a loop ends"
        );
    }
}
