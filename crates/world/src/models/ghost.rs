use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::{AssetId, UntypedAssetId};
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::ModelBlend;

use super::{DoodadLight, Furnished, Placed, batch_look, cached_form};
use crate::billboard::BillboardCard;
use crate::model::{ModelSubmesh, submesh_mesh};
use crate::model_material::{GroundShade, ModelMaterial, ModelMaterials, Variant, ghost_twin_of};
use crate::sight::Seen;
use crate::visibility::{ModelPart, alpha_bits};

pub(crate) struct GhostSpawner<'a, 'w, 's> {
    pub(crate) commands: &'a mut Commands<'w, 's>,
    pub(crate) meshes: &'a mut Assets<Mesh>,
    pub(crate) materials: &'a mut Assets<ModelMaterial>,
    pub(crate) cache: &'a mut ModelMaterials,
    pub(crate) twins: &'a mut HashMap<AssetId<ModelMaterial>, Handle<ModelMaterial>>,
    pub(crate) furnished: &'a mut Furnished,
    pub(crate) light: &'a Buffer,
}

pub(crate) struct GhostBatches {
    pub(crate) entities: Vec<Entity>,
    pub(crate) meshes_held: Arc<[Handle<Mesh>]>,
}

impl GhostSpawner<'_, '_, '_> {
    /// A model's batches at `transform`, drawn as the client draws a body faded to `alpha`.
    pub(crate) fn spawn(
        &mut self,
        model: UntypedAssetId,
        submeshes: &[ModelSubmesh],
        is_wmo: bool,
        transform: &Transform,
        alpha: f32,
    ) -> GhostBatches {
        let form = cached_form(&mut self.furnished.forms, self.meshes, model, |meshes| {
            submeshes
                .iter()
                .map(|s| meshes.add(submesh_mesh(&s.geometry)))
                .collect()
        });
        let unseen = |_| Seen::Terrain { column: 0, row: 0 };
        let placed = Placed {
            model,
            transform,
            is_wmo,
            light: DoodadLight::Sky(GroundShade::Lit),
            radius: f32::INFINITY,
            local_center: Vec3::ZERO,
            seen_by_batch: &unseen,
        };
        let mut out = Vec::with_capacity(2 * submeshes.len());
        for (i, (sub, mesh)) in submeshes.iter().zip(form.iter()).enumerate() {
            let look = batch_look(sub, i, &placed, GroundShade::Lit, false, None);
            let g = &sub.geometry;
            let multiplies = matches!(g.blend, ModelBlend::Mod | ModelBlend::Mod2x);
            let variant = if multiplies || g.blend == ModelBlend::Blend {
                Variant::Steady
            } else {
                Variant::FadeTwin
            };
            let primes = !(g.no_depth_write || g.no_depth_test || multiplies);
            let prime = primes.then_some(Variant::DepthPrime);
            for variant in std::iter::once(variant).chain(prime) {
                let drawn = self.cache.get(self.materials, &look, variant, self.light);
                let Some(material) = self.twin(&drawn) else {
                    continue;
                };
                let mut e = self.commands.spawn((
                    ModelPart,
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material),
                    MeshTag(alpha_bits(alpha)),
                ));
                match &sub.billboard {
                    Some(info) => {
                        let pivot = transform.transform_point(info.pivot);
                        e.insert((
                            Transform::from_translation(pivot),
                            BillboardCard::new(info, transform),
                        ));
                    }
                    None => {
                        e.insert(*transform);
                    }
                }
                if let Some(aabb) = sub.aabb {
                    e.insert((aabb, NoAutoAabb));
                }
                out.push(e.id());
            }
        }
        GhostBatches {
            entities: out,
            meshes_held: form,
        }
    }

    fn twin(&mut self, drawn: &Handle<ModelMaterial>) -> Option<Handle<ModelMaterial>> {
        if let Some(twin) = self.twins.get(&drawn.id()) {
            return Some(twin.clone());
        }
        let twin = self
            .materials
            .add(ghost_twin_of(self.materials.get(drawn)?));
        self.twins.insert(drawn.id(), twin.clone());
        Some(twin)
    }
}
