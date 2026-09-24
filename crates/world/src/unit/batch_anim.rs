use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::ecs::system::SystemParam;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use model::RenderSubmesh;

use super::body::BodyPart;
use super::fade::PartMaterials;
use crate::billboard::BillboardCard;
use crate::doodad_anim::{AnimMatPart, MatAnim, MatLoop, UvAnimMaterials, register};
use crate::mat_anim_table::MatAnimTable;
use crate::model::BillboardInfo;
use crate::model_material::ModelMaterial;
use crate::rig::RigPose;

#[derive(Component, Default)]
pub(crate) struct UnitCards(pub(crate) Vec<Entity>);

#[derive(Component)]
pub(crate) struct UnitAlphaAnimated;

pub(crate) fn unit_parts<'a>(
    root: Entity,
    children: &'a Query<'_, '_, &Children>,
    cards: Option<&'a UnitCards>,
) -> impl Iterator<Item = Entity> + 'a {
    let cards = cards.map_or(&[][..], |c| &c.0[..]);
    children.iter_descendants(root).chain(cards.iter().copied())
}

#[derive(SystemParam)]
pub(crate) struct UnitLoops<'w> {
    time: Res<'w, Time>,
    uv: ResMut<'w, UvAnimMaterials>,
    table: ResMut<'w, MatAnimTable>,
}

impl UnitLoops<'_> {
    pub(crate) fn register_scroll(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        part_materials: &PartMaterials,
        g: &RenderSubmesh,
    ) -> bool {
        let Some(scroll) = g.uv_anim.as_ref().filter(|a| a.period > 0.0) else {
            return false;
        };
        let scroll = Arc::new(scroll.clone());
        for id in part_materials.every_material() {
            let anim = MatLoop::Shared(scroll.clone());
            register(&mut self.uv, &mut self.table, materials, id, anim);
        }
        true
    }

    pub(crate) fn body_alpha(&self, g: &RenderSubmesh, unit: Entity) -> Option<MatAnim> {
        let anim = Arc::new(g.alpha_anim.clone()?);
        Some(MatAnim::following(anim, unit, self.time.elapsed_secs_f64()))
    }

    pub(crate) fn worn_alpha(&self, g: &RenderSubmesh) -> Option<MatAnim> {
        let anim = Arc::new(g.alpha_anim.clone()?);
        Some(MatAnim::new(anim, self.time.elapsed_secs_f64()))
    }
}

pub(crate) fn mark_animated(e: &mut EntityCommands<'_>, scrolls: bool, alpha: Option<MatAnim>) {
    if scrolls {
        e.insert(AnimMatPart);
    }
    if let Some(alpha) = alpha {
        e.insert(alpha);
    }
}

pub(crate) fn card_joint(
    commands: &mut Commands<'_, '_>,
    pose: Option<&mut RigPose>,
    owner: Entity,
    info: &BillboardInfo,
) -> Entity {
    if let Some(joint) = pose.and_then(|p| p.anchor_for(commands, info.bone)) {
        return joint;
    }
    commands
        .spawn((
            Transform::from_translation(info.pivot),
            Visibility::default(),
            ChildOf(owner),
        ))
        .id()
}

pub(crate) fn spawn_card<'a>(
    commands: &'a mut Commands<'_, '_>,
    mesh: Handle<Mesh>,
    tag: u32,
    part_materials: PartMaterials,
    info: &BillboardInfo,
    joint: Entity,
    aabb: Option<Aabb>,
) -> EntityCommands<'a> {
    let mut card = commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(part_materials.steady().clone()),
        MeshTag(tag),
        BillboardCard::following_joint(info.kind, joint),
        BodyPart,
        part_materials,
    ));
    if let Some(aabb) = aabb {
        card.insert(aabb);
    }
    card
}
