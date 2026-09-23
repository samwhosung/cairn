use bevy::mesh::MeshTag;
use bevy::prelude::*;
use model::CharSkinSlot;

use super::MeshCache;
use super::body::{BodyModel, BodyPart, HoldMeshes, WornModel, batch_look, meshes_for};
use super::fade::PartFade;
use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model_material::{ModelMaterial, ModelMaterials, Variant};
use crate::rig::RigPose;
use crate::source::{Repeat, m2_url, texture_url};
use crate::visibility::alpha_bits;

#[derive(Component)]
pub(crate) struct WornPending(Vec<(WornModel, Handle<M2Model>)>);

impl WornPending {
    pub(crate) fn new(worn: Vec<WornModel>, server: &AssetServer) -> Self {
        Self(
            worn.into_iter()
                .map(|w| {
                    let h = server.load(m2_url(&w.model));
                    (w, h)
                })
                .collect(),
        )
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn attach_worn(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Model>>,
    light: Option<Res<'_, LightBuffer>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut mesh_cache: ResMut<'_, MeshCache>,
    mut units: Query<'_, '_, (Entity, &mut WornPending, &BodyModel, &mut RigPose)>,
) {
    let Some(light) = light else {
        return;
    };
    for (entity, mut pending, body, mut pose) in &mut units {
        let Some(host) = m2s.get(&body.0) else {
            continue;
        };
        pending.0.retain(|(worn, handle)| {
            if !server.is_loaded_with_dependencies(handle) && !server.load_state(handle).is_failed()
            {
                return true;
            }
            let Some(item) = m2s.get(handle) else {
                return false;
            };
            let Some(point) = host.attachments.iter().find(|a| a.id == worn.attachment) else {
                return false;
            };
            let Some(anchor) = pose.anchor_for(&mut commands, point.bone) else {
                return false;
            };
            let form = meshes_for(&mut mesh_cache, &mut meshes, handle.id(), &item.submeshes);
            let object = worn
                .object_texture
                .as_deref()
                .map(|t| server.load::<Image>(texture_url(t, Repeat::BOTH)));
            let root = commands
                .spawn((
                    Transform::from_translation(point.offset),
                    Visibility::default(),
                    ChildOf(anchor),
                    HoldMeshes(form.clone()),
                ))
                .id();
            for (i, sub) in item.submeshes.iter().enumerate() {
                if sub.billboard.is_some() {
                    continue;
                }
                let g = &sub.geometry;
                let texture = if g.char_slot == Some(CharSkinSlot::Object) {
                    object.clone()
                } else {
                    sub.texture.clone()
                };
                let look = batch_look(g, texture, i);
                let material = cache.get(&mut materials, &look, Variant::Steady, &light.0);
                let fade = PartFade::of(
                    &mut cache,
                    &mut materials,
                    &look,
                    material.clone(),
                    false,
                    &light.0,
                );
                let mut part = commands.spawn((
                    Mesh3d(form.static_meshes[i].clone()),
                    MeshMaterial3d(material),
                    Transform::default(),
                    ChildOf(root),
                    MeshTag(alpha_bits(1.0)),
                    BodyPart,
                    fade,
                ));
                if let Some(aabb) = sub.aabb {
                    part.insert(aabb);
                }
            }
            false
        });
        if pending.0.is_empty() {
            commands.entity(entity).remove::<WornPending>();
        }
    }
}
