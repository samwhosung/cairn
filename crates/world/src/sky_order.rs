use bevy::render::render_resource::RenderPipelineDescriptor;

use crate::view::PROJECTION_FAR;

pub(crate) const SKY_VERTEX_SHADER: &str = "embedded://world/sky_vertex.wgsl";

pub(crate) const CLOUDS_SORT_RUNG: f32 = -6.0e5;

/// A rung rides the material's depth bias, which also offsets the rasterized depth; a sky pipeline
/// takes it back, so the pinned far depth reaches the depth test as it is.
pub(crate) fn sky_pipeline_state(descriptor: &mut RenderPipelineDescriptor) {
    if let Some(depth) = descriptor.depth_stencil.as_mut() {
        depth.depth_write_enabled = false;
        depth.bias.constant = 0;
    }
}

const _: () = {
    let world = 0.0;
    let rungs = [CLOUDS_SORT_RUNG, world];
    let mut i = 1;
    while i < rungs.len() {
        assert!(rungs[i] - rungs[i - 1] > PROJECTION_FAR);
        i += 1;
    }
};
