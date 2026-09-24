use bevy::render::render_resource::RenderPipelineDescriptor;

use crate::view::PROJECTION_FAR;

pub(crate) const SKY_VERTEX_SHADER: &str = "embedded://world/sky_vertex.wgsl";

pub(crate) const STARS_SORT_RUNG: f32 = -1.0e6;
pub(crate) const SUN_DISC_SORT_RUNG: f32 = -8.2e5;
pub(crate) const WHITE_MOON_SORT_RUNG: f32 = -8.1e5;
pub(crate) const SECOND_MOON_SORT_RUNG: f32 = -8.0e5;
pub(crate) const CLOUDS_SORT_RUNG: f32 = -6.0e5;
pub(crate) const GLARE_SORT_RUNG: f32 = 2.0e4;

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
    let rungs = [
        STARS_SORT_RUNG,
        SUN_DISC_SORT_RUNG,
        WHITE_MOON_SORT_RUNG,
        SECOND_MOON_SORT_RUNG,
        CLOUDS_SORT_RUNG,
        world,
        GLARE_SORT_RUNG,
    ];
    let mut i = 1;
    while i < rungs.len() {
        assert!(rungs[i] - rungs[i - 1] > PROJECTION_FAR);
        i += 1;
    }
};
