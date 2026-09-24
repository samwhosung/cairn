use bevy::prelude::*;
use model::CharSkinSlot;

use super::MeshCache;
use super::batch_anim::{
    UnitAlphaAnimated, UnitCards, UnitLoops, card_joint, mark_animated, spawn_card,
};
use super::body::{BodyModel, HoldMeshes, WornModel, batch_look, meshes_for, spawn_part};
use super::fade::PartMaterials;
use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model_material::{ModelMaterial, ModelMaterials, Variant};
use crate::rig::{RigPalettes, RigPose, RigSkin};
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
    mut palettes: ResMut<'_, RigPalettes>,
    mut loops: UnitLoops<'_>,
    mut units: Query<
        '_,
        '_,
        (
            Entity,
            &mut WornPending,
            &BodyModel,
            &mut RigPose,
            &mut UnitCards,
        ),
    >,
) {
    let Some(light) = light else {
        return;
    };
    for (entity, mut pending, body, mut pose, mut cards) in &mut units {
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
            let rig = rig_for_welded_billboards(item, root, &mut palettes);
            let slot = rig.as_ref().map_or(0, |(_, skin)| skin.slot);
            let mut alpha_animated = false;
            for (i, sub) in item.submeshes.iter().enumerate() {
                let g = &sub.geometry;
                let texture = if g.char_slot == Some(CharSkinSlot::Object) {
                    object.clone()
                } else {
                    sub.texture.clone()
                };
                let look = batch_look(g, texture, i, handle.id());
                let material = cache.get(&mut materials, &look, Variant::Steady, &light.0);
                let mats =
                    PartMaterials::of(&mut cache, &mut materials, &look, material, false, &light.0);
                let scrolls = loops.register_scroll(&mut materials, &mats, g);
                let alpha = loops.worn_alpha(g);
                alpha_animated |= alpha.is_some();
                if let Some(info) = &sub.billboard {
                    let joint = card_joint(&mut commands, None, root, info);
                    let tag = alpha_bits(1.0);
                    let mesh = form.static_meshes[i].clone();
                    let mut card =
                        spawn_card(&mut commands, mesh, tag, mats, info, joint, sub.aabb);
                    mark_animated(&mut card, scrolls, alpha);
                    cards.0.push(card.id());
                    continue;
                }
                let skinned = slot != 0;
                let mut part = spawn_part(&mut commands, root, &form, i, skinned, slot, mats);
                if !skinned && let Some(aabb) = sub.aabb {
                    part.insert(aabb);
                }
                mark_animated(&mut part, scrolls, alpha);
            }
            if let Some(rig) = rig {
                commands.entity(root).insert(rig);
            }
            if alpha_animated {
                commands.entity(entity).insert(UnitAlphaAnimated);
            }
            false
        });
        if pending.0.is_empty() {
            commands.entity(entity).remove::<WornPending>();
        }
    }
}

fn rig_for_welded_billboards(
    item: &M2Model,
    root: Entity,
    palettes: &mut RigPalettes,
) -> Option<(RigPose, RigSkin)> {
    let welded = item.submeshes.iter().any(|s| s.geometry.welded_billboard);
    if !welded || item.skeleton.joints.is_empty() {
        return None;
    }
    let skin = RigSkin::allocate(palettes, item.inverse_bindposes.clone())?;
    Some((RigPose::new(root, &item.skeleton), skin))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use model::{BillboardKind, RenderSubmesh};

    use super::*;
    use crate::model::ModelSubmesh;
    use crate::rig::{ModelJoint, ModelSkeleton};

    fn item(welded: bool, bones: usize) -> M2Model {
        let joint = |parent, billboard| ModelJoint {
            parent,
            local_translation: Vec3::ZERO,
            billboard,
            parent_arm: None,
        };
        let joints = [joint(-1, None), joint(0, Some(BillboardKind::Spherical))];
        M2Model {
            submeshes: vec![ModelSubmesh {
                geometry: Arc::new(RenderSubmesh {
                    welded_billboard: welded,
                    ..RenderSubmesh::default()
                }),
                aabb: None,
                texture: None,
                billboard: None,
            }],
            bounds: None,
            lights: Vec::new(),
            skeleton: ModelSkeleton {
                joints: joints[..bones].to_vec(),
                spine_bone: None,
                head_bone: None,
            },
            inverse_bindposes: vec![Mat4::IDENTITY; bones].into(),
            attachments: Vec::new(),
            animations: None,
            has_emitters: false,
            emitters: Vec::new(),
            ribbons: Vec::new(),
        }
    }

    #[test]
    fn only_an_item_welded_to_a_billboard_bone_takes_a_palette() {
        let mut palettes = RigPalettes::default();
        let root = Entity::PLACEHOLDER;
        let (pose, skin) =
            rig_for_welded_billboards(&item(true, 2), root, &mut palettes).expect("a rig");
        assert_eq!((pose.joints_root, pose.locals.len()), (root, 2));
        assert_ne!(skin.slot, 0);
        assert!(rig_for_welded_billboards(&item(false, 2), root, &mut palettes).is_none());
        assert!(rig_for_welded_billboards(&item(true, 0), root, &mut palettes).is_none());
    }
}
