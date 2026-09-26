use bevy::render::render_resource::RenderPipelineDescriptor;

use crate::view::{FARCLIP, PROJECTION_FAR};

pub(crate) const SKY_VERTEX_SHADER: &str = "embedded://world/sky_vertex.wgsl";

pub(crate) const STARS_SORT_RUNG: f32 = -1.0e6;
pub(crate) const SUN_DISC_SORT_RUNG: f32 = -8.2e5;
pub(crate) const WHITE_MOON_SORT_RUNG: f32 = -8.1e5;
pub(crate) const SECOND_MOON_SORT_RUNG: f32 = -8.0e5;
pub(crate) const CLOUDS_SORT_RUNG: f32 = -6.0e5;
pub(crate) const SKYBOX_SORT_RUNG: f32 = -6.0e4;
/// The client draws the ground decals after the sky and before the water and every other
/// transparent: a unit's shadow first, the footprints over it.
pub(crate) const SHADOW_SORT_RUNG: f32 = -5.2e4;
pub(crate) const FOOTPRINT_SORT_RUNG: f32 = -5.0e4;
/// Ground clutter's depth, laid before any of its colour.
pub(crate) const CLUTTER_DEPTH_SORT_RUNG: f32 = -4.69e4;
/// Ground clutter blends over the ground and its decals, and the water over the clutter under it.
pub(crate) const CLUTTER_SORT_RUNG: f32 = -4.38e4;
pub(crate) const FAR_SIDE_SORT_RUNG: f32 = -4.0e4;
pub(crate) const WATER_SORT_RUNG: f32 = -2.0e4;
pub(crate) const FOAM_SORT_RUNG: f32 = -1.0e4;
pub(crate) const DRIFT_SORT_RUNG: f32 = 1.4e4;
pub(crate) const GLARE_SORT_RUNG: f32 = 2.0e4;

/// The rasterizer's depth bias on every ground decal: its vertices, cut and placed on the CPU, lie
/// on the drawn ground only to within rounding.
pub(crate) const DECAL_RASTER: i32 = 32768;

/// Bevy also puts a material's depth bias in its pipeline key, truncated to an integer: a painted
/// sky's batches step apart by an amount f32 keeps at the rung's magnitude, and all truncate to
/// the rung's one key.
const SKYBOX_ORDER_STEP: f32 = 1.0 / 64.0;
const SKYBOX_ORDER_CAP: f32 = 0.9;
/// A band's batches start this far under their rung, so their order steps, below one, all
/// truncate to the band's one pipeline key.
pub(crate) const BAND_DROP: f32 = 0.99;

pub(crate) fn skybox_batch_bias(batch_order: f32) -> f32 {
    SKYBOX_SORT_RUNG - BAND_DROP + (batch_order * SKYBOX_ORDER_STEP).min(SKYBOX_ORDER_CAP)
}

/// A rung rides the material's depth bias, which also offsets the rasterized depth; a draw placed
/// by its rung takes it back, so its depth reaches the depth test as it is.
pub(crate) fn sort_only(descriptor: &mut RenderPipelineDescriptor) {
    if let Some(depth) = descriptor.depth_stencil.as_mut() {
        depth.bias.constant = 0;
    }
}

pub(crate) fn sky_pipeline_state(descriptor: &mut RenderPipelineDescriptor) {
    sort_only(descriptor);
    if let Some(depth) = descriptor.depth_stencil.as_mut() {
        depth.depth_write_enabled = false;
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
        SKYBOX_SORT_RUNG,
        CLUTTER_DEPTH_SORT_RUNG,
        CLUTTER_SORT_RUNG,
        FAR_SIDE_SORT_RUNG,
        WATER_SORT_RUNG,
        FOAM_SORT_RUNG,
        world,
        DRIFT_SORT_RUNG,
        GLARE_SORT_RUNG,
    ];
    let mut i = 1;
    while i < rungs.len() {
        assert!(rungs[i] - rungs[i - 1] > PROJECTION_FAR);
        i += 1;
    }
    let band_floor = SKYBOX_SORT_RUNG - BAND_DROP;
    assert!(band_floor as i32 == (band_floor + SKYBOX_ORDER_CAP) as i32);
    assert!(band_floor + SKYBOX_ORDER_STEP > band_floor);
    // The effect lane discards what lies past the far clip, so two decals seen together keep
    // their order.
    assert!(SHADOW_SORT_RUNG - SKYBOX_SORT_RUNG > PROJECTION_FAR);
    assert!(FOOTPRINT_SORT_RUNG - SHADOW_SORT_RUNG > FARCLIP);
    assert!(CLUTTER_DEPTH_SORT_RUNG - FOOTPRINT_SORT_RUNG > PROJECTION_FAR);
};
