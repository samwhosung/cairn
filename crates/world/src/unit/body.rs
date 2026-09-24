use std::collections::HashMap;
use std::num::NonZeroU16;
use std::sync::{Arc, Weak};

use bevy::animation::graph::AnimationGraphHandle;
use bevy::animation::transition::AnimationTransitions;
use bevy::asset::AssetId;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use model::{CharSkinSlot, FogPolicy, ModelBlend, RenderSubmesh};

use crate::light::LightBuffer;
use crate::m2::M2Model;
use crate::model::{ModelSubmesh, skinned_submesh_mesh, submesh_mesh};
use crate::model_material::{
    BatchId, BatchLook, GroundShade, ModelMaterial, ModelMaterials, Variant,
};
use crate::particles::{EmitClock, EmitterFrames, OwnerLoss, spawn_emitter};
use crate::ribbons::{RibbonSeq, spawn_ribbon};
use crate::rig::{GlobalSeqDrive, RigPalettes, RigPose, RigSkin};
use crate::source::{Repeat, m2_url, texture_url};
use crate::visibility::alpha_bits;

use super::batch_anim::{
    UnitAlphaAnimated, UnitCards, UnitLoops, card_joint, mark_animated, spawn_card,
};
use super::fade::{PartMaterials, UnitAppear};

/// A body to draw at this entity's transform: its model, the creature skins it fills from its
/// display, and for a character the textures and geosets its appearance chose.
#[derive(Component, Clone, Debug)]
pub struct UnitBody {
    /// The model's path as the tables name it.
    pub model: String,
    /// The `Monster1..3` skin names, found beside the model.
    pub skins: [Option<String>; 3],
    pub character: Option<CharacterDress>,
}

/// A character body's runtime textures and the geosets it shows. `None` leaves a slot's batches
/// with the model's own, untextured look.
#[derive(Clone, Debug, Default)]
pub struct CharacterDress {
    pub body: Option<Handle<Image>>,
    pub hair: Option<Handle<Image>>,
    pub skin_extra: Option<Handle<Image>>,
    pub object: Option<Handle<Image>>,
    pub geosets: Vec<u16>,
    /// The item models it wears at attachment points: a helm, a pair of pauldrons.
    pub worn: Vec<WornModel>,
}

/// An item model worn at one of the body's attachment points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WornModel {
    pub model: String,
    /// The skin its runtime object batches take; a path.
    pub object_texture: Option<String>,
    pub attachment: u16,
}

/// On a dressed body: its mesh parts, not its billboard cards, and the rig slot they skin
/// through, 0 for none.
#[derive(Component, Debug)]
pub struct BodyDressed {
    pub parts: Vec<Entity>,
    pub slot: u16,
}

/// The model a body draws, once it has been asked for.
#[derive(Component)]
pub struct BodyModel(pub Handle<M2Model>);

/// Each model's meshes, shared by every body wearing it: static and skinned per batch.
#[derive(Resource, Default)]
pub(crate) struct MeshCache(HashMap<AssetId<M2Model>, Weak<ModelMeshes>>);

pub(crate) struct ModelMeshes {
    pub(crate) static_meshes: Vec<Handle<Mesh>>,
    skinned_meshes: Vec<Handle<Mesh>>,
}

#[derive(Component)]
pub(crate) struct HoldMeshes(
    #[allow(dead_code, reason = "held so the meshes live")] pub(crate) Arc<ModelMeshes>,
);

const RIG_SHIFT: u32 = 19;

pub(crate) fn rig_bits(slot: u16) -> u32 {
    u32::from(slot) << RIG_SHIFT
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn dress_bodies(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Model>>,
    light: Option<Res<'_, LightBuffer>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut cache: ResMut<'_, ModelMaterials>,
    mut mesh_cache: ResMut<'_, MeshCache>,
    mut palettes: ResMut<'_, RigPalettes>,
    mut loops: UnitLoops<'_>,
    bodies: Query<
        '_,
        '_,
        (Entity, &UnitBody, Option<&BodyModel>, Option<&Transform>),
        Without<BodyDressed>,
    >,
) {
    let Some(light) = light else {
        return;
    };
    for (entity, body, handle, placement) in &bodies {
        let handle = if let Some(BodyModel(h)) = handle {
            h.clone()
        } else {
            let h = server.load(m2_url(&body.model));
            commands.entity(entity).insert(BodyModel(h.clone()));
            h
        };
        let Some(m2) = m2s.get(&handle) else {
            continue;
        };
        let form = meshes_for(&mut mesh_cache, &mut meshes, handle.id(), &m2.submeshes);
        let rigged = !m2.skeleton.joints.is_empty();
        let skin = (rigged && !m2.submeshes.is_empty())
            .then(|| RigSkin::allocate(&mut palettes, m2.inverse_bindposes.clone()))
            .flatten();
        let slot = skin.as_ref().map_or(0, |s| s.slot);
        let dir = model_dir(&body.model);
        let hair_part = m2
            .submeshes
            .iter()
            .find(|s| s.geometry.char_slot == Some(CharSkinSlot::Hair))
            .map(|s| (s.geometry.blend, s.geometry.two_sided));
        let mut pose = rigged.then(|| RigPose::new(entity, &m2.skeleton));
        let (mut parts, mut cards, mut alpha_animated) = (Vec::new(), Vec::new(), false);
        for (i, sub) in m2.submeshes.iter().enumerate() {
            let g = &sub.geometry;
            if let Some(dress) = &body.character
                && !dress.geosets.contains(&g.geoset_id)
            {
                continue;
            }
            let PartLook {
                look,
                runtime_sheet,
            } = part_look(sub, i, body, &server, dir, hair_part, handle.id());
            let material = cache.get(&mut materials, &look, Variant::Steady, &light.0);
            let mats = PartMaterials::of(
                &mut cache,
                &mut materials,
                &look,
                material.clone(),
                runtime_sheet,
                &light.0,
            );
            let scrolls = loops.register_scroll(&mut materials, &mats, g);
            let alpha = loops.body_alpha(g, entity);
            alpha_animated |= alpha.is_some();
            if let Some(info) = &sub.billboard {
                let joint = card_joint(&mut commands, pose.as_mut(), entity, info);
                let tag = rig_bits(slot) | alpha_bits(1.0);
                let mesh = form.static_meshes[i].clone();
                let mut card = spawn_card(&mut commands, mesh, tag, mats, info, joint, sub.aabb);
                mark_animated(&mut card, scrolls, alpha);
                cards.push(card.id());
                continue;
            }
            let skinned = slot != 0 && rigged;
            let mut part = spawn_part(&mut commands, entity, &form, i, skinned, slot, mats);
            if !skinned && let Some(aabb) = sub.aabb {
                part.insert(aabb);
            }
            mark_animated(&mut part, scrolls, alpha);
            parts.push(part.id());
        }
        let worn = body
            .character
            .as_ref()
            .map_or_else(Vec::new, |c| c.worn.clone());
        spawn_effects(
            &mut commands,
            m2,
            entity,
            placement.copied().unwrap_or_default(),
            pose.as_mut(),
        );
        let mut root = commands.entity(entity);
        root.insert((
            BodyDressed { parts, slot },
            HoldMeshes(form),
            UnitAppear::at(time.elapsed_secs()),
            UnitCards(cards),
        ));
        if alpha_animated {
            root.insert(UnitAlphaAnimated);
        }
        if !worn.is_empty() {
            root.insert(super::attach::WornPending::new(worn, &server));
        }
        insert_rig_and_players(&mut root, m2, skin, pose);
    }
}

fn spawn_part<'a>(
    commands: &'a mut Commands<'_, '_>,
    owner: Entity,
    form: &ModelMeshes,
    index: usize,
    skinned: bool,
    slot: u16,
    part_materials: PartMaterials,
) -> EntityCommands<'a> {
    let mesh = if skinned {
        &form.skinned_meshes[index]
    } else {
        &form.static_meshes[index]
    };
    let mut part = commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(part_materials.steady().clone()),
        Transform::default(),
        ChildOf(owner),
        MeshTag(rig_bits(slot) | alpha_bits(1.0)),
        BodyPart,
        part_materials,
    ));
    if skinned {
        part.insert(NoFrustumCulling);
    }
    part
}

fn spawn_effects(
    commands: &mut Commands<'_, '_>,
    m2: &M2Model,
    entity: Entity,
    placement: Transform,
    mut pose: Option<&mut RigPose>,
) {
    for em in &m2.emitters {
        let owner = pose
            .as_deref_mut()
            .and_then(|p| p.anchor_for(commands, em.def.bone))
            .map_or((entity, [0.0; 3]), |j| (j, em.bone_pivot));
        let frames = EmitterFrames {
            owner: Some(owner),
            anchor: Some(entity),
            alpha: Some(entity),
            on_owner_loss: OwnerLoss::Free,
        };
        spawn_emitter(commands, em, placement, frames, EmitClock::Host(entity));
    }
    for rb in &m2.ribbons {
        let (owner, use_pivot) = pose
            .as_deref_mut()
            .and_then(|p| p.anchor_for(commands, rb.def.bone))
            .map_or((entity, false), |j| (j, true));
        spawn_ribbon(
            commands,
            rb,
            owner,
            use_pivot,
            placement.scale.max_element(),
            RibbonSeq::Host(entity),
            Some(entity),
            None,
        );
    }
}

fn insert_rig_and_players(
    root: &mut EntityCommands<'_>,
    m2: &M2Model,
    skin: Option<RigSkin>,
    pose: Option<RigPose>,
) {
    let skeleton = &m2.skeleton;
    if let Some(pose) = pose {
        root.insert(pose);
        if let Some(skin) = skin {
            root.insert(skin);
            let bone = |b: Option<u16>| b.filter(|&b| usize::from(b) < skeleton.joints.len());
            let (spine, head) = (bone(skeleton.spine_bone), bone(skeleton.head_bone));
            if spine.is_some() || head.is_some() {
                root.insert(super::BodyTwist::new(spine, head));
            }
        }
    }
    let rigged = !skeleton.joints.is_empty();
    if let Some(anims) = &m2.animations {
        if rigged
            && let Some(drive) = GlobalSeqDrive::new(&anims.global_bones, m2.skeleton.joints.len())
        {
            root.insert(drive);
        }
        root.insert((
            AnimationPlayer::default(),
            AnimationGraphHandle(anims.graph.clone()),
            anims.clone(),
            AnimationTransitions::new(),
            super::UnitDriver::default(),
        ));
    }
}

/// On every mesh a body spawned.
#[derive(Component)]
pub struct BodyPart;

pub(crate) fn meshes_for(
    cache: &mut MeshCache,
    meshes: &mut Assets<Mesh>,
    id: AssetId<M2Model>,
    submeshes: &[ModelSubmesh],
) -> Arc<ModelMeshes> {
    if let Some(f) = cache.0.get(&id).and_then(Weak::upgrade) {
        return f;
    }
    let f = Arc::new(ModelMeshes {
        static_meshes: submeshes
            .iter()
            .map(|s| meshes.add(submesh_mesh(&s.geometry)))
            .collect(),
        skinned_meshes: submeshes
            .iter()
            .map(|s| meshes.add(skinned_submesh_mesh(&s.geometry)))
            .collect(),
    });
    cache.0.insert(id, Arc::downgrade(&f));
    f
}

struct PartLook {
    look: BatchLook,
    runtime_sheet: bool,
}

fn part_look(
    sub: &ModelSubmesh,
    index: usize,
    body: &UnitBody,
    server: &AssetServer,
    dir: &str,
    hair_part: Option<(ModelBlend, bool)>,
    model: AssetId<M2Model>,
) -> PartLook {
    let g: &RenderSubmesh = &sub.geometry;
    if let (Some(slot), Some(dress)) = (g.char_slot, &body.character) {
        let char_look = |texture: &Option<Handle<Image>>, blend: ModelBlend, two_sided: bool| {
            texture.clone().map(|t| character_look(t, blend, two_sided))
        };
        let look = match slot {
            // The client draws the body opaque, and every hair batch as it draws the first.
            CharSkinSlot::Body => char_look(&dress.body, ModelBlend::Opaque, g.two_sided),
            CharSkinSlot::Hair => {
                hair_part.and_then(|(blend, two_sided)| char_look(&dress.hair, blend, two_sided))
            }
            CharSkinSlot::Object => char_look(&dress.object, g.blend, g.two_sided),
            CharSkinSlot::SkinExtra => char_look(&dress.skin_extra, g.blend, g.two_sided),
        };
        if let Some(look) = look {
            return PartLook {
                look,
                runtime_sheet: true,
            };
        }
    }
    let texture = match (&sub.texture, g.skin_slot) {
        (Some(t), _) => Some(t.clone()),
        (None, Some(slot)) if g.char_slot.is_none() => body
            .skins
            .get(slot as usize)
            .and_then(Option::as_ref)
            .map(|name| server.load(texture_url(&format!("{dir}\\{name}.blp"), Repeat::BOTH))),
        _ => None,
    };
    PartLook {
        look: batch_look(g, texture, index, model),
        runtime_sheet: false,
    }
}

fn scrolling_batch(g: &RenderSubmesh, model: AssetId<M2Model>, index: usize) -> Option<BatchId> {
    g.uv_anim
        .as_ref()
        .filter(|a| a.period > 0.0)
        .map(|_| BatchId {
            model: model.untyped(),
            index,
        })
}

pub(crate) fn batch_look(
    g: &RenderSubmesh,
    texture: Option<Handle<Image>>,
    index: usize,
    model: AssetId<M2Model>,
) -> BatchLook {
    BatchLook {
        texture,
        blend: g.blend,
        two_sided: g.two_sided,
        is_wmo: false,
        interior: g.interior,
        emissive: g.emissive,
        additive: g.additive,
        no_depth_write: g.no_depth_write,
        no_depth_test: g.no_depth_test,
        fog_policy: g.fog_policy,
        env_map: g.env_map,
        shade: GroundShade::Entity,
        batch_order: Some(NonZeroU16::MIN.saturating_add(u16::try_from(index).unwrap_or(u16::MAX))),
        uv_offset_at_rest: g.uv_anim.as_ref().map_or([0.0, 0.0], |a| a.sample(0.0)),
        tint_at_rest: g.rgb_anim.as_ref().map_or([1.0; 3], |a| a.sample(0.0)),
        animated: scrolling_batch(g, model, index),
        seq_owner: None,
        wmo_class: None,
        sidn: None,
        window: false,
        skybox: false,
    }
}

fn character_look(texture: Handle<Image>, blend: ModelBlend, two_sided: bool) -> BatchLook {
    BatchLook {
        texture: Some(texture),
        blend,
        two_sided,
        is_wmo: false,
        interior: false,
        emissive: false,
        additive: false,
        no_depth_write: false,
        no_depth_test: false,
        fog_policy: FogPolicy::Scene,
        env_map: false,
        shade: GroundShade::Entity,
        batch_order: None,
        uv_offset_at_rest: [0.0, 0.0],
        tint_at_rest: [1.0; 3],
        animated: None,
        seq_owner: None,
        wmo_class: None,
        sidn: None,
        window: false,
        skybox: false,
    }
}

pub(crate) fn model_dir(path: &str) -> &str {
    path.rsplit_once(['\\', '/']).map_or("", |(dir, _)| dir)
}
