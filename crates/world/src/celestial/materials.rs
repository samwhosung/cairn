use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use super::SPRITE_SPHERE_YARDS;
use crate::sky_order::{SKY_VERTEX_SHADER, sky_pipeline_state};

pub type CelestialMaterial = ExtendedMaterial<StandardMaterial, CelestialExtension>;

#[derive(Asset, AsBindGroup, Clone, TypePath, Default)]
pub struct CelestialExtension {
    #[uniform(100)]
    pub(super) look: SpriteLook,
    #[uniform(101)]
    pub(super) span: DiscSpan,
}

#[derive(Clone, Copy, Default, PartialEq, Debug, ShaderType)]
pub(super) struct SpriteLook {
    pub horizon_slope: f32,
    pub glare: u32,
    pub disc_alpha: f32,
}

#[derive(Clone, Copy, Default, PartialEq, Debug, ShaderType)]
pub(super) struct DiscSpan {
    pub sin_bottom: f32,
    pub sin_top: f32,
}

impl MaterialExtension for CelestialExtension {
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/celestial/celestial.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}

pub(super) const DISC_HORIZON_FADE: f32 = 2.5 * SPRITE_SPHERE_YARDS;

pub type StarMaterial = ExtendedMaterial<StandardMaterial, StarExtension>;

#[derive(Asset, AsBindGroup, Clone, TypePath, Default)]
pub struct StarExtension {}

impl MaterialExtension for StarExtension {
    fn vertex_shader() -> ShaderRef {
        SKY_VERTEX_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/celestial/star.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        sky_pipeline_state(descriptor);
        Ok(())
    }
}
