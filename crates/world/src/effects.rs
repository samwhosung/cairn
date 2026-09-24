//! The effect stream: one vertex stream a frame that every dynamic effect writes into, and the
//! transparent draws over it, each sorted by its own anchor.

mod lane;
mod stream;

use bevy::prelude::*;

pub use stream::{
    EffectBatch, EffectBlend, EffectDraw, EffectDrawSpec, EffectFog, EffectLighting, EffectQuads,
    EffectTopology, EffectVertex, WorldEffectDraw, begin_effect_frame,
};

pub(crate) struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(lane::EffectRenderPlugin)
            .init_resource::<EffectQuads>()
            .add_systems(
                PostUpdate,
                begin_effect_frame.after(bevy::transform::TransformSystems::Propagate),
            )
            .add_systems(Last, stream::clear_effect_frame_flag);
    }
}
