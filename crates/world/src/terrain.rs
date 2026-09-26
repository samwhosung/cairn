use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::{ShaderDefVal, ShaderRef};

use crate::adt::AdtTile;
use crate::draw_order::OrderedMaterialPlugin;
use crate::sight::frame::SIGHT_GROUND;

pub type TerrainMaterial = ExtendedMaterial<StandardMaterial, TerrainExtension>;

/// One sampler, the layer array's, serves all three arrays: the shader declares only one.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(TerrainKey)]
pub struct TerrainExtension {
    #[texture(100, dimension = "2d_array", visibility(fragment))]
    #[sampler(105, visibility(fragment))]
    pub layer_array: Handle<Image>,
    #[texture(104, dimension = "2d_array", visibility(fragment))]
    pub alpha_array: Handle<Image>,
    #[texture(110, dimension = "2d_array", visibility(fragment))]
    pub shadow_array: Handle<Image>,
    #[uniform(106)]
    pub params: Vec4,
    #[storage(90, read_only, buffer)]
    pub light: Buffer,
    pub sight: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerrainKey {
    sight: bool,
}

impl From<&TerrainExtension> for TerrainKey {
    fn from(e: &TerrainExtension) -> Self {
        Self { sight: e.sight }
    }
}

impl MaterialExtension for TerrainExtension {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/terrain.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/terrain.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.sight
            && let Some(fragment) = descriptor.fragment.as_mut()
        {
            fragment.shader_defs.push("WOW_SIGHT".into());
            let ground = ShaderDefVal::UInt("WOW_SIGHT_GROUND".into(), SIGHT_GROUND);
            fragment.shader_defs.push(ground);
        }
        Ok(())
    }
}

pub(crate) struct TerrainMaterialPlugin;

impl Plugin for TerrainMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "terrain.wgsl");
        app.add_plugins(OrderedMaterialPlugin::<TerrainMaterial>::default());
    }
}

pub(crate) fn sight_twin_of(drawn: &TerrainMaterial) -> TerrainMaterial {
    let mut sight = drawn.clone();
    sight.extension.sight = true;
    sight
}

/// Back faces culled, as the client culls terrain.
pub(crate) fn terrain_material(tile: &AdtTile, light: &Buffer) -> TerrainMaterial {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 1.0,
            double_sided: false,
            cull_mode: Some(Face::Back),
            ..StandardMaterial::default()
        },
        extension: TerrainExtension {
            layer_array: tile.layer_array.clone(),
            alpha_array: tile.alpha_array.clone(),
            shadow_array: tile.shadow_array.clone(),
            params: Vec4::new(terrain::LAYER_REPEATS_PER_CHUNK, 0.0, 0.0, 0.0),
            light: light.clone(),
            sight: false,
        },
    }
}
