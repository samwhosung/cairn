use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::prelude::*;

use super::lazy::{LazyRig, SkinnedTwin};
use super::{DoodadAnimHost, DrawBounds, Gate, HostBuilder, SeenBy, spawn_anim_host};
use crate::billboard::BillboardCard;
use crate::m2::M2Model;
use crate::model::BillboardInfo;

pub(crate) struct RigBuilder {
    host: HostBuilder,
    skinned: Option<Arc<[Handle<Mesh>]>>,
    ibp: Arc<[Mat4]>,
    bound: Option<Aabb>,
    bounds: DrawBounds,
    now: f32,
    batches: Vec<Entity>,
    lazy_parts: Vec<Entity>,
}

impl RigBuilder {
    pub(crate) fn spawn(
        commands: &mut Commands<'_, '_>,
        m: &M2Model,
        transform: &Transform,
        skinned: impl FnOnce() -> Arc<[Handle<Mesh>]>,
        bounds: DrawBounds,
        now: f32,
    ) -> Option<Self> {
        if !m.has_emitters && m.submeshes.iter().all(|s| s.billboard.is_some()) {
            return None;
        }
        let host = spawn_anim_host(commands, m, *transform)?;
        Some(Self {
            host,
            skinned: Some(skinned()),
            ibp: m.inverse_bindposes.clone(),
            bound: m.animated_bound(),
            bounds,
            now,
            batches: Vec::new(),
            lazy_parts: Vec::new(),
        })
    }

    pub(crate) fn card(
        &mut self,
        commands: &mut Commands<'_, '_>,
        info: &BillboardInfo,
    ) -> Option<BillboardCard> {
        let anchor = self.host.anchor(commands, info.bone)?;
        Some(BillboardCard::following_joint(info.kind, anchor))
    }

    pub(crate) fn add_batch(
        &mut self,
        e: &mut EntityCommands<'_>,
        index: usize,
        unskinned: &Handle<Mesh>,
        aabb: Option<Aabb>,
    ) {
        let bound = match (aabb, self.bound) {
            (Some(s), Some(a)) => Some(Aabb::from_min_max(
                Vec3::from(s.min().min(a.min())),
                Vec3::from(s.max().max(a.max())),
            )),
            (s, a) => s.or(a),
        };
        if let Some(bound) = bound {
            e.insert((bound, NoAutoAabb));
        }
        if let Some(skinned) = self.skinned.as_ref().and_then(|s| s.get(index)) {
            e.insert(SkinnedTwin {
                skinned: skinned.clone(),
                unskinned: unskinned.clone(),
            });
            self.lazy_parts.push(e.id());
        }
        self.batches.push(e.id());
    }

    #[must_use]
    pub(crate) fn finish(self, commands: &mut Commands<'_, '_>) -> Entity {
        let root = self.host.root;
        commands.entity(root).insert(DoodadAnimHost {
            seen_by: if self.batches.is_empty() {
                SeenBy::Bounds(self.bounds)
            } else {
                SeenBy::Batches(self.batches)
            },
            clip: self.host.clip,
            armed_at: self.now,
            rerolls_at: f32::NEG_INFINITY,
            anim_id: self.host.anim_id,
            gate: Gate::New,
            parked_at: self.now,
        });
        if !self.lazy_parts.is_empty() {
            commands.entity(root).insert(LazyRig {
                ibp: self.ibp,
                parts: self.lazy_parts,
            });
        }
        self.host.finish(commands);
        root
    }
}
