//! Skinned, animated models: the pose sampled from the playing sequences, composed down the bone
//! chain, and written as skinning frames the model shader blends.

mod anims;
mod bake;
mod compose;
mod global_seq;
mod palette;
mod pose;
mod rng;
mod source;

use bevy::prelude::*;

pub use anims::{AnimClip, ClipEvent, ModelAnimations, ResolvedAnim};
pub use bake::{
    GlobalBone, GlobalSeqChannel, ModelAttachment, ModelJoint, ModelSkeleton, build_animation_clip,
    build_attachments, build_global_bones, build_skeleton, skeleton_pivots,
};
pub(crate) use compose::seed_rig_rows;
pub use compose::{PosePost, RigFinalize};
pub use global_seq::GlobalSeqDrive;
pub use palette::{BONE_BYTES, MAX_PALETTE_BONES, MAX_RIG_SLOTS, RigPalettes, RigSkin};
pub use pose::RigPose;
#[cfg(test)]
pub(crate) use pose::plugin as pose_evaluation;
pub use rng::AnimRng;
pub use source::{PoseBone, PoseClip, PoseNode, PoseSource, PoseTrack};

/// On a rig nobody sees: nothing samples, composes or finalizes its pose until the marker goes, and
/// its global sequences then take up the world's clock where it stands.
#[derive(Component)]
pub struct AnimParked;

pub(crate) struct RigPlugin;

impl Plugin for RigPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimRng>();
        pose::plugin(app);
        compose::plugin(app);
        global_seq::plugin(app);
        palette::plugin(app);
    }
}
