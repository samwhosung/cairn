use bevy::asset::embedded_asset;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

use crate::draw_order::OrderedMaterialPlugin;
use crate::sky_order::SCREEN_MARKS_SORT_RUNG;

#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(MarkSpace)]
pub(crate) struct MarkMaterial {
    #[uniform(0)]
    pub colour: Vec4,
    pub space: MarkSpace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MarkSpace {
    ScreenOnTop,
    WorldDepthTested,
}

impl From<&MarkMaterial> for MarkSpace {
    fn from(m: &MarkMaterial) -> Self {
        m.space
    }
}

impl Material for MarkMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/hands/marks.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/hands/marks.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn depth_bias(&self) -> f32 {
        match self.space {
            MarkSpace::ScreenOnTop => SCREEN_MARKS_SORT_RUNG,
            MarkSpace::WorldDepthTested => 0.0,
        }
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        crate::sky_order::sort_only(descriptor);
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = false;
        }
        if key.bind_group_data == MarkSpace::ScreenOnTop {
            descriptor.vertex.shader_defs.push("MARK_IN_NDC".into());
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_compare = CompareFunction::Always;
            }
        }
        Ok(())
    }
}

pub(crate) struct MarksPlugin;

impl Plugin for MarksPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "marks.wgsl");
        app.add_plugins(OrderedMaterialPlugin::<MarkMaterial>::default());
    }
}
