//! Reads World of Warcraft 1.12.1 M2 models: mesh, embedded skins, bones, attachments, tracks.

mod camera;
mod error;
mod model;
mod parse;
mod skin;
mod track;

pub use camera::{parse_camera_lookup, parse_cameras};
pub use error::Error;
pub use model::{
    C2, C3, M2ArrayString, M2Attachment, M2BlendMode, M2Bone, M2BoneFlags, M2Bounds, M2Camera,
    M2EventMarker, M2Format, M2Material, M2Model, M2PlayableAnim, M2RawData, M2RenderFlags,
    M2Texture, M2TextureTransform, M2TextureType, M2Vertex,
};
pub use parse::parse_m2;
pub use skin::{Skin, SkinBatch, SkinSection};
pub use track::{
    CubicValue, M2QuatTrack, M2ScalarSplineTrack, M2ScalarTrack, M2SplineKey, M2Track,
    M2Vec3SplineTrack, M2Vec3Track,
};

#[cfg(test)]
mod tests;
