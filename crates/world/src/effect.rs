//! Effect quads rebuilt each frame: textured, vertex coloured, unlit, alpha blended or added,
//! drawn in the transparent pass at a fixed place in its sort.

use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

pub type EffectMaterial = ExtendedMaterial<StandardMaterial, EffectExtension>;

/// How an effect draws.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct EffectLook {
    /// Adds to what is behind it rather than blending over it.
    pub additive: bool,
    /// Fogs with the scene.
    pub fogged: bool,
    /// Its vertices are relative to the camera.
    pub camera_relative: bool,
    /// Its place in the transparent sort, added to its distance.
    pub sort: f32,
    /// Its rasterizer depth bias, toward the eye.
    pub raster_bias: i32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectKey {
    raster_bias: i32,
}

impl From<&EffectExtension> for EffectKey {
    fn from(e: &EffectExtension) -> Self {
        Self {
            raster_bias: e.raster_bias,
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(EffectKey)]
pub struct EffectExtension {
    #[texture(100)]
    #[sampler(101)]
    pub texture: Handle<Image>,
    #[uniform(102)]
    pub params: Vec4,
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light: Buffer,
    raster_bias: i32,
}

impl MaterialExtension for EffectExtension {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/effect.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/effect.wgsl".into()
    }

    /// The sort place rides the material's depth bias; the rasterizer takes the draw's own.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.bias.constant = key.bind_group_data.raster_bias;
            ds.bias.slope_scale = 0.0;
        }
        Ok(())
    }
}

pub(crate) struct EffectPlugin;

impl Plugin for EffectPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "effect.wgsl");
        app.add_plugins(MaterialPlugin::<EffectMaterial>::default());
    }
}

pub(crate) fn effect_material(
    look: EffectLook,
    texture: Handle<Image>,
    light: &Buffer,
) -> EffectMaterial {
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    ExtendedMaterial {
        base: StandardMaterial {
            unlit: true,
            alpha_mode: if look.additive {
                AlphaMode::Premultiplied
            } else {
                AlphaMode::Blend
            },
            cull_mode: None,
            double_sided: true,
            depth_bias: look.sort,
            ..StandardMaterial::default()
        },
        extension: EffectExtension {
            texture,
            params: Vec4::new(
                flag(look.fogged),
                flag(look.additive),
                flag(look.camera_relative),
                0.0,
            ),
            light: light.clone(),
            raster_bias: look.raster_bias,
        },
    }
}
