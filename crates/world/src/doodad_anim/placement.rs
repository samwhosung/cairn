use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use model::{KeyAnim, RenderSubmesh, SeqLoops};

use super::lazy::{LazyRig, SkinnedTwin};
use super::mat_anim::{AnimMatPart, MatLoop, TintAnimMaterials, UvAnimMaterials, register};
use super::{DoodadAnimHost, Gate, HostBuilder, MatAnim, SeenBy, spawn_anim_host};
use crate::billboard::BillboardCard;
use crate::m2::M2Model;
use crate::mat_anim_table::MatAnimTable;
use crate::model::BillboardInfo;
use crate::model_material::ModelMaterial;
use crate::particles::DrawSetGate;

#[derive(SystemParam)]
pub(crate) struct MaterialLoops<'w> {
    time: Res<'w, Time>,
    uv: ResMut<'w, UvAnimMaterials>,
    tint: ResMut<'w, TintAnimMaterials>,
    table: ResMut<'w, MatAnimTable>,
}

fn mat_loop<V: Clone>(
    shared: Option<&KeyAnim<V>>,
    per_seq: Option<&SeqLoops<V>>,
    seq_owner: Option<Entity>,
) -> Option<MatLoop<V>> {
    match (per_seq, seq_owner) {
        (Some(seqs), Some(seq_owner)) => Some(MatLoop::PerSeq {
            seqs: Arc::new(seqs.clone()),
            seq_owner,
        }),
        _ => shared
            .filter(|a| a.period > 0.0)
            .map(|a| MatLoop::Shared(Arc::new(a.clone()))),
    }
}

impl MaterialLoops<'_> {
    pub(crate) fn now(&self) -> f32 {
        self.time.elapsed_secs()
    }

    pub(crate) fn register(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        cutout: AssetId<ModelMaterial>,
        fade_twin: AssetId<ModelMaterial>,
        g: &RenderSubmesh,
        seq_owner: Option<Entity>,
    ) {
        let uv = mat_loop(g.uv_anim.as_ref(), g.uv_seq.as_ref(), seq_owner);
        let tint = mat_loop(g.rgb_anim.as_ref(), g.rgb_seq.as_ref(), seq_owner);
        for id in [cutout, fade_twin] {
            if let Some(anim) = uv.clone() {
                register(&mut self.uv, &mut self.table, materials, id, anim);
            }
            if let Some(anim) = tint.clone() {
                register(&mut self.tint, &mut self.table, materials, id, anim);
            }
        }
    }

    pub(crate) fn loops_for(&self, g: &RenderSubmesh, seq_owner: Option<Entity>) -> PartLoops {
        let moves = |p: Option<f32>| p.is_some_and(|p| p > 0.0);
        PartLoops {
            animated_material: moves(g.uv_anim.as_ref().map(|a| a.period))
                || moves(g.rgb_anim.as_ref().map(|a| a.period))
                || seq_owner.is_some(),
            alpha: g
                .alpha_anim
                .as_ref()
                .map(|a| MatAnim::new(Arc::new(a.clone()), self.time.elapsed_secs_f64())),
        }
    }
}

pub(crate) struct PartLoops {
    animated_material: bool,
    alpha: Option<MatAnim>,
}

impl PartLoops {
    pub(crate) fn insert(self, e: &mut EntityCommands<'_>) {
        if self.animated_material {
            e.insert(AnimMatPart);
        }
        if let Some(alpha) = self.alpha {
            e.insert(alpha);
        }
    }
}

pub(crate) struct RigBuilder {
    host: HostBuilder,
    skinned: Option<Arc<[Handle<Mesh>]>>,
    ibp: Arc<[Mat4]>,
    bound: Option<Aabb>,
    bounds: DrawSetGate,
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
        bounds: DrawSetGate,
        now: f32,
    ) -> Option<Self> {
        if !m.has_emitters && m.submeshes.iter().all(|s| s.billboard.is_some()) {
            return None;
        }
        let host = spawn_anim_host(commands, m, *transform)?;
        Some(Self {
            skinned: host.skins.then(skinned),
            host,
            ibp: m.inverse_bindposes.clone(),
            bound: m.animated_bound(),
            bounds,
            now,
            batches: Vec::new(),
            lazy_parts: Vec::new(),
        })
    }

    pub(crate) fn root(&self) -> Entity {
        self.host.root
    }

    pub(crate) fn bone_anchor(
        &mut self,
        commands: &mut Commands<'_, '_>,
        bone: u16,
    ) -> Option<Entity> {
        self.host.anchor(commands, bone)
    }

    pub(crate) fn sequence_player(&self) -> Option<Entity> {
        self.host.plays_idle_on_player.then_some(self.host.root)
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
            own_stream: None,
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
