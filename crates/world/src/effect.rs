use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

pub type EffectMaterial = ExtendedMaterial<StandardMaterial, EffectExtension>;

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct EffectLook {
    pub additive: bool,
    pub fogged: bool,
    pub camera_relative: bool,
    pub sort_offset: f32,
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

/// `effect.wgsl`'s `EffectParams`, member for member.
#[derive(Clone, Copy, Default, Debug, ShaderType)]
pub struct EffectParams {
    pub fogged: f32,
    pub additive: f32,
    pub camera_relative: f32,
    pub _pad: f32,
}

#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(EffectKey)]
pub struct EffectExtension {
    #[texture(100)]
    #[sampler(101)]
    pub texture: Handle<Image>,
    #[uniform(102)]
    pub params: EffectParams,
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
            depth_bias: look.sort_offset,
            ..StandardMaterial::default()
        },
        extension: EffectExtension {
            texture,
            params: EffectParams {
                fogged: flag(look.fogged),
                additive: flag(look.additive),
                camera_relative: flag(look.camera_relative),
                _pad: 0.0,
            },
            light: light.clone(),
            raster_bias: look.raster_bias,
        },
    }
}
