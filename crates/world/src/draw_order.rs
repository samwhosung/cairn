use std::hash::Hash;
use std::marker::PhantomData;

use bevy::ecs::system::StaticSystemParam;
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::erased_render_asset::{
    ErasedRenderAsset, ExtractedAssets as NewMaterials, prepare_erased_assets,
};
use bevy::render::render_asset::prepare_assets;
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSystems};

/// Bevy's material plugin for `M`, with `M`'s bind groups handed out and freed in id order.
pub(crate) struct OrderedMaterialPlugin<M>(PhantomData<M>);

impl<M> Default for OrderedMaterialPlugin<M> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<M: Material> Plugin for OrderedMaterialPlugin<M>
where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    #[expect(clippy::disallowed_types)]
    fn build(&self, app: &mut App) {
        app.add_plugins(bevy::pbr::MaterialPlugin::<M>::default());
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(
                Render,
                bind_groups_in_id_order::<M>
                    .in_set(RenderSystems::PrepareAssets)
                    .after(prepare_assets::<GpuImage>)
                    .before(prepare_erased_assets::<MeshMaterial3d<M>>),
            );
        }
    }
}

/// Bevy numbers a frame's new material bind groups, and frees its removed ones for reuse, in a hash
/// order that takes in the type's id, which changes with the build; on one opaque pipeline the
/// higher-numbered bind group wins an exact depth tie. Bevy's unload, run again on `removed`, then
/// frees nothing. A material prepared before its images waits a frame and is numbered ahead of that
/// frame's, so the images are prepared first.
fn bind_groups_in_id_order<M: Material>(
    mut extracted: ResMut<'_, NewMaterials<MeshMaterial3d<M>>>,
    unload: StaticSystemParam<'_, '_, <MeshMaterial3d<M> as ErasedRenderAsset>::Param>,
) where
    M::Data: PartialEq + Eq + Hash + Clone,
{
    extracted.extracted.sort_unstable_by_key(|(id, _)| *id);
    let mut removed: Vec<_> = extracted.removed.iter().copied().collect();
    removed.sort_unstable();
    let mut unload = unload.into_inner();
    for id in removed {
        MeshMaterial3d::<M>::unload_asset(id, &mut unload);
    }
}
