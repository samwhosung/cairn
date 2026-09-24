use std::sync::Arc;

use bevy::mesh::MeshTag;
use bevy::prelude::*;

use std::num::NonZeroU16;

use super::{DoodadAnimHost, Gate};
use crate::rig::{RigPalettes, RigPose, RigSkin, seed_rig_rows};
use crate::visibility::with_rig;

const REAP_LOW_WATER: usize = 256;
const REAP_PER_FRAME: usize = 64;
const REAP_MIN_PARKED_SECS: f32 = 2.0;

#[derive(Component)]
pub(crate) struct LazyRig {
    pub(crate) ibp: Arc<[Mat4]>,
    pub(crate) parts: Vec<Entity>,
}

#[derive(Component)]
pub(crate) struct SkinnedTwin {
    pub(crate) skinned: Handle<Mesh>,
    pub(crate) unskinned: Handle<Mesh>,
}

pub(super) type TwinParts<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Mesh3d,
        &'static mut MeshTag,
        &'static SkinnedTwin,
    ),
>;

pub(super) fn promote_lazy_rig(
    commands: &mut Commands<'_, '_>,
    palettes: &mut RigPalettes,
    worlds: &Query<'_, '_, &GlobalTransform>,
    root: Entity,
    lazy: &LazyRig,
    pose: Option<&RigPose>,
    parts: &mut TwinParts<'_, '_>,
) {
    let Some(rig) = RigSkin::allocate(palettes, lazy.ibp.clone()) else {
        return;
    };
    let slot = rig.slot;
    if let Some(pose) = pose {
        let root_g = worlds.get(root).copied().unwrap_or_default();
        seed_rig_rows(pose, root_g, &rig, palettes);
    }
    commands.queue(move |world: &mut World| match world.get_entity_mut(root) {
        Ok(mut e) => {
            e.insert(rig);
        }
        Err(_) => world.resource_mut::<RigPalettes>().free(slot),
    });
    for &part in &lazy.parts {
        let Ok((mut mesh, mut tag, twin)) = parts.get_mut(part) else {
            continue;
        };
        mesh.0 = twin.skinned.clone();
        tag.0 = with_rig(tag.0, NonZeroU16::new(slot));
    }
}

fn demote_lazy_rig(
    commands: &mut Commands<'_, '_>,
    root: Entity,
    lazy: &LazyRig,
    parts: &mut TwinParts<'_, '_>,
) {
    for &part in &lazy.parts {
        let Ok((mut mesh, mut tag, twin)) = parts.get_mut(part) else {
            continue;
        };
        mesh.0 = twin.unskinned.clone();
        tag.0 = with_rig(tag.0, None);
    }
    commands.queue(move |world: &mut World| {
        if let Ok(mut e) = world.get_entity_mut(root) {
            e.remove::<RigSkin>();
        }
    });
}

pub(super) fn reap_parked_rigs(
    time: Res<'_, Time>,
    palettes: Res<'_, RigPalettes>,
    hosts: Query<'_, '_, (Entity, &DoodadAnimHost, &LazyRig), With<RigSkin>>,
    mut parts: TwinParts<'_, '_>,
    mut commands: Commands<'_, '_>,
) {
    if palettes.free_slots() >= REAP_LOW_WATER {
        return;
    }
    let now = time.elapsed_secs();
    let mut parked: Vec<(f32, Entity)> = hosts
        .iter()
        .filter(|(_, host, _)| {
            host.gate == Gate::Parked && now - host.parked_at >= REAP_MIN_PARKED_SECS
        })
        .map(|(root, host, _)| (host.parked_at, root))
        .collect();
    parked.sort_by(|a, b| a.0.total_cmp(&b.0));
    for &(_, root) in parked.iter().take(REAP_PER_FRAME) {
        if let Ok((_, _, lazy)) = hosts.get(root) {
            demote_lazy_rig(&mut commands, root, lazy, &mut parts);
        }
    }
}
