use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;

use super::query::{LiquidClaim, LiquidGrid, surfaces_at};
use super::{Underwater, WaterIndex};
use crate::coords::bevy_to_wow;
use crate::model_material::{ModelMaterial, far_twin_of};
use crate::visibility::DoodadFade;

/// Bevy sorts each transparent draw at one distance, so the side of the water a batch draws on is
/// its material's place in the sort: the far side takes a twin a rung below the water.
#[derive(Resource, Default)]
pub(crate) struct FarSide {
    to_far: HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
    to_near: HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
    far: HashSet<Entity>,
}

impl FarSide {
    pub(crate) fn sided<'a>(
        &'a self,
        entity: Entity,
        want: &'a Handle<ModelMaterial>,
    ) -> &'a Handle<ModelMaterial> {
        if self.far.contains(&entity) {
            self.to_far.get(&want.id()).unwrap_or(want)
        } else {
            want
        }
    }

    fn twin(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        near: &Handle<ModelMaterial>,
    ) -> Option<Handle<ModelMaterial>> {
        if let Some(far) = self.to_far.get(&near.id()) {
            return Some(far.clone());
        }
        let far = materials.add(far_twin_of(materials.get(near)?));
        self.to_far.insert(near.id(), far.clone());
        self.to_near.insert(far.id(), near.clone());
        Some(far)
    }
}

fn height_over_water(
    index: &WaterIndex,
    grids: &Query<'_, '_, &LiquidGrid>,
    at: Vec3,
) -> Option<f32> {
    let wow = bevy_to_wow(at);
    let near = index
        .0
        .over(wow[0], wow[1])
        .iter()
        .filter_map(|&e| grids.get(e).ok());
    surfaces_at(near, wow, LiquidClaim::Unknown)
        .map(|z| wow[2] - z)
        .min_by(|a, b| a.abs().total_cmp(&b.abs()))
}

fn far_side(
    index: &WaterIndex,
    grids: &Query<'_, '_, &LiquidGrid>,
    at: Vec3,
    eye_submerged: bool,
) -> bool {
    let point_submerged = height_over_water(index, grids, at).is_some_and(|d| d < 0.0);
    point_submerged != eye_submerged
}

type Part<'a> = (
    Entity,
    &'a GlobalTransform,
    &'a mut MeshMaterial3d<ModelMaterial>,
    Option<&'a DoodadFade>,
);

fn takes_a_side(material: Option<&ModelMaterial>) -> bool {
    material.is_some_and(|m| {
        matches!(m.base.alpha_mode, AlphaMode::Blend)
            && !m.extension.is_wmo()
            && !m.extension.is_clutter()
    })
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn classify_water_side(
    index: Res<'_, WaterIndex>,
    grids: Query<'_, '_, &LiquidGrid>,
    underwater: Res<'_, Underwater>,
    mut side: ResMut<'_, FarSide>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut set: ParamSet<
        '_,
        '_,
        (
            Query<
                '_,
                '_,
                Entity,
                (
                    With<MeshMaterial3d<ModelMaterial>>,
                    Or<(
                        Changed<GlobalTransform>,
                        Changed<MeshMaterial3d<ModelMaterial>>,
                    )>,
                ),
            >,
            Query<'_, '_, Part<'_>>,
        ),
    >,
    mut was_submerged: Local<'_, Option<bool>>,
) {
    let submerged = underwater.0.any();
    let everything = *was_submerged != Some(submerged) || index.is_changed();
    *was_submerged = Some(submerged);
    let dirty: Vec<Entity> = if everything {
        set.p1().iter().map(|p| p.0).collect()
    } else {
        set.p0().iter().collect()
    };
    let mut parts = set.p1();
    for entity in dirty {
        let Ok((entity, at, mut material, fade)) = parts.get_mut(entity) else {
            continue;
        };
        let near = side
            .to_near
            .get(&material.0.id())
            .cloned()
            .unwrap_or_else(|| material.0.clone());
        let fade_owns_handle = fade.is_some();
        let decides = fade.map_or(&near, |f| &f.blend);
        let far = takes_a_side(materials.get(decides))
            && far_side(&index, &grids, at.translation(), submerged);
        let twin = if far {
            side.far.insert(entity);
            side.twin(&mut materials, decides)
        } else {
            side.far.remove(&entity);
            None
        };
        if !fade_owns_handle {
            let want = twin.unwrap_or(near);
            if material.0 != want {
                material.0 = want;
            }
        }
    }
    side.far.retain(|&e| parts.contains(e));
}
