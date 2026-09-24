//! What of a unit's model moves besides its bones: the batches that turn to face the camera, the
//! texture scrolls, and the alpha each batch is authored with per sequence.

use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::ecs::system::SystemParam;
use bevy::mesh::MeshTag;
use bevy::prelude::*;
use model::RenderSubmesh;

use super::body::BodyPart;
use super::fade::PartFade;
use crate::billboard::BillboardCard;
use crate::doodad_anim::{AnimMatPart, MatAnim, MatLoop, UvAnimMaterials, register};
use crate::mat_anim_table::MatAnimTable;
use crate::model::BillboardInfo;
use crate::model_material::ModelMaterial;
use crate::rig::RigPose;

/// A unit's billboard batches: world roots that follow its joints, drawn as its parts are.
#[derive(Component, Default)]
pub(crate) struct UnitCards(pub(crate) Vec<Entity>);

/// A unit with a batch whose alpha moves, redrawn every frame.
#[derive(Component)]
pub(crate) struct UnitAlphaAnimated;

#[derive(SystemParam)]
pub(crate) struct UnitLoops<'w> {
    time: Res<'w, Time>,
    uv: ResMut<'w, UvAnimMaterials>,
    table: ResMut<'w, MatAnimTable>,
}

impl UnitLoops<'_> {
    /// A batch's texture scroll runs on every material it can be drawn with, shared by every unit
    /// of the model. The client runs no tint loop on a unit's batches.
    pub(crate) fn register(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        fade: &PartFade,
        g: &RenderSubmesh,
    ) -> bool {
        let Some(scroll) = g.uv_anim.as_ref().filter(|a| a.period > 0.0) else {
            return false;
        };
        let scroll = Arc::new(scroll.clone());
        for id in fade.materials() {
            let anim = MatLoop::Shared(scroll.clone());
            register(&mut self.uv, &mut self.table, materials, id, anim);
        }
        true
    }

    /// A batch of the unit's own body reads the sequence the unit plays; one of an item it wears
    /// rests in the item's first sequence.
    pub(crate) fn alpha(&self, g: &RenderSubmesh, host: Option<Entity>) -> Option<MatAnim> {
        let anim = Arc::new(g.alpha_anim.clone()?);
        let now = self.time.elapsed_secs_f64();
        Some(match host {
            Some(host) => MatAnim::following(anim, host, now),
            None => MatAnim::new(anim, now),
        })
    }
}

/// Marks a part or card with what of it moves, and says whether its alpha does.
pub(crate) fn mark_moving(
    e: &mut EntityCommands<'_>,
    scrolls: bool,
    alpha: Option<MatAnim>,
) -> bool {
    if scrolls {
        e.insert(AnimMatPart);
    }
    let animated = alpha.is_some();
    if let Some(alpha) = alpha {
        e.insert(alpha);
    }
    animated
}

/// The joint a card rides: its bone's on a rigged body, else a point at its pivot under `owner`.
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

/// A card is drawn on its own, a world root the facing system places, not under the body.
pub(crate) fn spawn_card<'a>(
    commands: &'a mut Commands<'_, '_>,
    mesh: Handle<Mesh>,
    tag: u32,
    fade: PartFade,
    info: &BillboardInfo,
    joint: Entity,
    aabb: Option<Aabb>,
) -> EntityCommands<'a> {
    let mut card = commands.spawn((
        Mesh3d(mesh),
        MeshMaterial3d(fade.steady().clone()),
        MeshTag(tag),
        BillboardCard::following_joint(info.kind, joint),
        BodyPart,
        fade,
    ));
    if let Some(aabb) = aabb {
        card.insert(aabb);
    }
    card
}
