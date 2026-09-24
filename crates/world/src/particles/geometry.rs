use bevy::asset::AssetId;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use super::{GeometryParticleMesh, ParticleEmitter};
use crate::coords::wow_to_bevy;
use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model::submesh_mesh;
use crate::model_material::{ModelMaterial, ModelMaterials, Variant};
use crate::unit::batch_look;
use crate::visibility::alpha_bits;

const MAX_INSTANCES: usize = 128;

pub(super) struct GeometryInstance {
    pub(super) tinted_meshes: Vec<(Entity, Handle<ModelMaterial>)>,
}

#[derive(Resource, Default)]
pub(super) struct GeometryMeshes(HashMap<AssetId<M2Model>, Vec<Handle<Mesh>>>);

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn update_model_particles(
    mut commands: Commands<'_, '_>,
    models: Res<'_, Assets<M2Model>>,
    mut forms: ResMut<'_, GeometryMeshes>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    light: Option<Res<'_, LightBuffer>>,
    mut emitters: Query<'_, '_, &mut ParticleEmitter>,
    mut draws: Query<
        '_,
        '_,
        (
            &mut Transform,
            &mut GlobalTransform,
            &mut Visibility,
            &mut MeshTag,
        ),
        With<GeometryParticleMesh>,
    >,
) {
    let Some(light) = light else {
        return;
    };
    for mut emitter in &mut emitters {
        let Some(geometry) = emitter.geometry.clone() else {
            continue;
        };
        if emitter.gated {
            continue;
        }
        let Some(model) = models.get(&geometry) else {
            continue;
        };
        let form = forms
            .0
            .entry(geometry.id())
            .or_insert_with(|| {
                model
                    .submeshes
                    .iter()
                    .map(|s| meshes.add(submesh_mesh(&s.geometry)))
                    .collect()
            })
            .clone();
        let rung = model::owner_last_rung_bucket(emitter.owner_rung());
        let want = emitter.particles.len().min(MAX_INSTANCES);
        while emitter.model_instances.len() < want {
            let slot = model
                .submeshes
                .iter()
                .zip(&form)
                .enumerate()
                .map(|(i, (sub, mesh))| {
                    let look = batch_look(&sub.geometry, sub.texture.clone(), i, geometry.id());
                    let shared = cache.get(&mut materials, &look, Variant::Steady, &light.0);
                    let mut own = materials.get(&shared).cloned();
                    if let Some(m) = own.as_mut()
                        && m.base.alpha_mode == AlphaMode::Blend
                    {
                        m.base.depth_bias = rung;
                    }
                    let material = own.map_or(shared, |m| materials.add(m));
                    let entity = commands
                        .spawn((
                            Mesh3d(mesh.clone()),
                            MeshMaterial3d(material.clone()),
                            Transform::IDENTITY,
                            Visibility::Hidden,
                            NoFrustumCulling,
                            MeshTag(alpha_bits(1.0)),
                            GeometryParticleMesh,
                        ))
                        .id();
                    (entity, material)
                })
                .collect();
            emitter.model_instances.push(GeometryInstance {
                tinted_meshes: slot,
            });
        }
        let world_space = !emitter.def.model_space();
        let inst_scale = if emitter.def.scale_size_by_instance() {
            emitter.placement.scale.x.max(1e-4)
        } else {
            1.0
        };
        for (i, slot) in emitter.model_instances.iter().enumerate() {
            let Some(p) = emitter.particles.get(i) else {
                for (e, _) in &slot.tinted_meshes {
                    if let Ok((_, _, mut vis, _)) = draws.get_mut(*e)
                        && *vis != Visibility::Hidden
                    {
                        *vis = Visibility::Hidden;
                    }
                }
                continue;
            };
            let ol = emitter
                .def
                .over_life
                .sample((p.age / p.life).clamp(0.0, 1.0));
            let (mut rgba, size) = (ol.color, ol.size);
            rgba[3] *= emitter.alpha;
            let tf = if world_space {
                Transform {
                    translation: p.pos,
                    rotation: p.quat,
                    scale: Vec3::splat(size * inst_scale),
                }
            } else {
                Transform {
                    translation: emitter
                        .placement
                        .transform_point(wow_to_bevy(p.pos.to_array())),
                    rotation: emitter.placement.rotation * p.quat,
                    scale: Vec3::splat(size * inst_scale),
                }
            };
            for (e, mat) in &slot.tinted_meshes {
                if let Ok((mut t, mut g, mut vis, mut tag)) = draws.get_mut(*e) {
                    *t = tf;
                    *g = GlobalTransform::from(tf);
                    if *vis != Visibility::Inherited {
                        *vis = Visibility::Inherited;
                    }
                    *tag = MeshTag(alpha_bits(rgba[3]));
                }
                if let Some(m) = materials.get_mut(mat) {
                    m.extension.tint = Vec4::new(rgba[0], rgba[1], rgba[2], m.extension.tint.w);
                }
            }
        }
    }
}
