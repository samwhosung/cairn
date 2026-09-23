use std::ffi::CString;

use crate::skin::SkinBatch;
use crate::track::{
    M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2Vec3SplineTrack, M2Vec3Track,
};

#[derive(Debug, Clone, Copy)]
pub struct C3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct C2 {
    pub x: f32,
    pub y: f32,
}

/// What a texture slot is for. `Hardcoded` names its file; the `Monster*` slots usually carry
/// no filename and are filled from the creature's display variation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum M2TextureType {
    Hardcoded,
    Monster1,
    Monster2,
    Monster3,
    Other(u32),
}

impl M2TextureType {
    pub(crate) fn from_u32(v: u32) -> Self {
        match v {
            0 => Self::Hardcoded,
            11 => Self::Monster1,
            12 => Self::Monster2,
            13 => Self::Monster3,
            other => Self::Other(other),
        }
    }
}

/// A texture filename, cut at its first NUL; empty when the file's string is out of range.
#[derive(Debug)]
pub struct M2ArrayString {
    pub string: CString,
}

#[derive(Debug)]
pub struct M2Texture {
    pub texture_type: M2TextureType,
    /// U repeats when set and clamps to the edge when clear: cutout cards author UVs outside
    /// `0..1` so the clamped border fades to the texture's transparent edge.
    pub wrap_x: bool,
    pub wrap_y: bool,
    pub filename: M2ArrayString,
}

/// Material render flags: `0x01` unlit, `0x04` two-sided.
#[derive(Debug, Clone, Copy)]
pub struct M2RenderFlags(pub(crate) u16);

impl M2RenderFlags {
    pub fn bits(&self) -> u16 {
        self.0
    }
}

/// Blend mode: 0 opaque, 1 alpha-key, 2 alpha, 3 and 4 additive, 5 modulate, 6 modulate 2x.
#[derive(Debug, Clone, Copy)]
pub struct M2BlendMode(pub(crate) u16);

impl M2BlendMode {
    pub fn bits(&self) -> u16 {
        self.0
    }
}

#[derive(Debug)]
pub struct M2Material {
    pub flags: M2RenderFlags,
    pub blend_mode: M2BlendMode,
}

/// Bone flags: `0x08` spherical billboard, `0x10`/`0x20`/`0x40` cylindrical billboard.
#[derive(Debug, Clone, Copy)]
pub struct M2BoneFlags(pub(crate) u32);

impl M2BoneFlags {
    pub fn bits(&self) -> u32 {
        self.0
    }
}

#[derive(Debug)]
pub struct M2Bone {
    /// The key-bone id, `-1` for none: `0`/`1` arms, `2`/`3` shoulders, `4` spine, `5` waist,
    /// `6` head and so on.
    pub key_bone: i16,
    pub flags: M2BoneFlags,
    /// The parent bone's index, `-1` for a root.
    pub parent: i16,
    /// The pivot, with a NaN component read as zero, as the client does.
    pub pivot: C3,
}

impl M2Bone {
    pub fn is_billboard(&self) -> bool {
        self.flags.0 & (0x08 | 0x10 | 0x20 | 0x40) != 0
    }
}

/// One vertex; the file's second UV set is not read.
#[derive(Debug)]
pub struct M2Vertex {
    pub position: C3,
    /// Influence weights out of 255, paired with `bone_indices`.
    pub bone_weights: [u8; 4],
    pub bone_indices: [u8; 4],
    pub normal: C3,
    pub tex_coords: C2,
}

/// The texture lookup table, and the collision hull as raw `u16` triangle indices and
/// three-`f32` vertices.
#[derive(Debug)]
pub struct M2RawData {
    pub texture_lookup_table: Vec<u16>,
    pub bounding_triangles: Vec<u8>,
    pub bounding_vertices: Vec<u8>,
}

/// The authored bounds: the bounding box and sphere cover every animation's vertices, the
/// collision box and sphere the model's body.
#[derive(Debug)]
pub struct M2Bounds {
    pub bounding_box_min: [f32; 3],
    pub bounding_box_max: [f32; 3],
    pub bounding_sphere_radius: f32,
    pub collision_box_min: [f32; 3],
    pub collision_box_max: [f32; 3],
    pub collision_sphere_radius: f32,
}

/// One row of the playable-animation lookup: for a requested `AnimationData.dbc` id, the id the
/// model plays instead, baked from that table's fallback chain against the model's sequences.
#[derive(Debug, Clone, Copy)]
pub struct M2PlayableAnim {
    pub resolved_id: u16,
    /// The row's high half, a direction or variant code.
    pub dir_flags: u16,
}

/// One attachment point, in the model's own space. Records whose id or bone does not fit `u16`,
/// or whose bone is out of range, are dropped.
#[derive(Debug, Clone, Copy)]
pub struct M2Attachment {
    pub id: u16,
    pub bone: u16,
    pub position: [f32; 3],
}

/// The position of one animation-event record, such as the `$CSL` cast-release point. Idents
/// repeat within a model, and the client takes the first match. Records whose bone does not fit
/// `u16` or is out of range are dropped.
#[derive(Debug, Clone, Copy)]
pub struct M2EventMarker {
    /// The identifier as it reads, `*b"$CSL"`.
    pub ident: [u8; 4],
    pub bone: u16,
    pub position: [f32; 3],
}

/// One texture transform: the UV animation behind waterfalls and scrolling energy fields.
#[derive(Debug)]
pub struct M2TextureTransform {
    pub translation: M2Vec3Track,
    pub rotation: M2QuatTrack,
    pub scaling: M2Vec3Track,
}

/// A fly-by's path, or a portrait's framing. The eye is `position_base + positions(t)` and the
/// target `target_base + target(t)`, so a keyless track leaves the base as the value.
#[derive(Debug, Clone)]
pub struct M2Camera {
    /// `0` portrait, `1` character info, `-1` on every shipped fly-by.
    pub camera_type: i32,
    /// The diagonal field of view in radians: the vertical half-angle is
    /// `(fov / 2) / sqrt(aspect² + 1)`.
    pub fov: f32,
    pub far_clip: f32,
    pub near_clip: f32,
    pub positions: M2Vec3SplineTrack,
    pub position_base: [f32; 3],
    pub target: M2Vec3SplineTrack,
    pub target_base: [f32; 3],
    pub roll: M2ScalarSplineTrack,
}

#[derive(Debug)]
pub struct M2Model {
    pub vertices: Vec<M2Vertex>,
    pub textures: Vec<M2Texture>,
    pub materials: Vec<M2Material>,
    /// One alpha track per colour record, picked directly by a batch's `color_index`. The
    /// client culls a batch whose alpha is at most zero.
    pub color_alpha_tracks: Vec<M2ScalarTrack>,
    /// One RGB tint per colour record, picked like the alpha; glow cards drawn with a neutral
    /// texture take their colour from it.
    pub color_rgb_tracks: Vec<M2Vec3Track>,
    /// Weight tracks, reached through `transparency_lookup[batch.weight_combo_index]`.
    pub transparency_tracks: Vec<M2ScalarTrack>,
    pub transparency_lookup: Vec<u16>,
    /// Where a batch stage's texture coordinates come from: `0..=2` a vertex UV set, anything
    /// higher a generated environment coordinate. See [`M2Model::stage_is_env_mapped`].
    pub texture_unit_lookup: Vec<u16>,
    /// Reached through `texture_transform_lookup[batch.texture_transform_combo_index]`.
    pub texture_transforms: Vec<M2TextureTransform>,
    /// `0xffff` for no transform.
    pub texture_transform_lookup: Vec<u16>,
    /// Loop lengths in milliseconds for tracks tagged with a global sequence.
    pub global_sequences: Vec<u32>,
    pub bones: Vec<M2Bone>,
    pub cameras: Vec<M2Camera>,
    /// A camera purpose index to its slot in `cameras`; portraits use entry 0.
    pub camera_lookup: Vec<u16>,
    pub raw_data: M2RawData,
    pub bounds: M2Bounds,
    /// Z of the file record attachment id 17 resolves to: the follow camera's pivot height.
    /// `None` when the model has no such attachment.
    pub pivot_attach_z: Option<f32>,
    /// Look records up by id through [`M2Model::attachment`], not by scanning: some models
    /// author several records under one id and the lookup picks one.
    pub attachments: Vec<M2Attachment>,
    /// Attachment id to an index into `attachments`, `0xffff` for none.
    pub attach_lookup: Vec<u16>,
    pub event_markers: Vec<M2EventMarker>,
    /// `AnimationData.dbc` id to the model's first sequence for it, `0xffff` for none. It ends
    /// just past the highest id the model authors.
    pub animation_lookup: Vec<u16>,
    pub playable_animation_lookup: Vec<M2PlayableAnim>,
    pub(crate) views: (u32, u32),
    pub(crate) version: u32,
}

impl M2Model {
    /// Whether `batch`'s texture stage `stage` uses generated environment coordinates rather
    /// than a vertex UV set. An index past the lookup table counts as generated, as in the
    /// client; models that rely on it author no usable UVs at all.
    pub fn stage_is_env_mapped(&self, batch: &SkinBatch, stage: u16) -> bool {
        let idx = batch.texture_coord_combo_index as usize + stage as usize;
        self.texture_unit_lookup.get(idx).is_none_or(|&v| v > 2)
    }

    /// Whether the model authors `AnimationData.dbc` id `anim_id`. An id past the end of the
    /// lookup is not owned.
    pub fn owns_animation(&self, anim_id: u16) -> bool {
        self.animation_lookup
            .get(anim_id as usize)
            .is_some_and(|&slot| slot != 0xffff)
    }

    /// The attachment attach id `id` resolves to, or `None`: the client hangs nothing on an
    /// unresolved id rather than falling back to the origin.
    pub fn attachment(&self, id: u16) -> Option<&M2Attachment> {
        let idx = *self.attach_lookup.get(id as usize)?;
        (idx != 0xffff)
            .then(|| self.attachments.get(idx as usize))
            .flatten()
    }
}

#[derive(Debug)]
pub struct M2Format {
    pub(crate) model: M2Model,
}

impl M2Format {
    pub fn model(&self) -> &M2Model {
        &self.model
    }
}
