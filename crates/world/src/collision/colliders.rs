//! Static colliders for streamed geometry, built off the main thread: building a trimesh's tree
//! is the cost that would hitch a frame, and attaching the finished shape is main-thread work, so
//! attaches are spread over frames under a budget.

use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, CollisionLayers, RigidBody};
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on};
use model::CollisionMesh;
use terrain::ChunkMesh;

use crate::coords::wow_to_bevy;

/// Wall-clock time spent attaching finished colliders per frame before the rest wait.
pub const ATTACH_BUDGET: Duration = Duration::from_millis(2);

/// Replaces [`ATTACH_BUDGET`]; zero attaches exactly one collider per call.
#[derive(Resource)]
pub struct AttachBudget(pub Duration);

/// A static collider building on the async compute pool, attached to this entity when done.
#[derive(Component)]
pub struct PendingCollider {
    task: Option<Task<Collider>>,
    /// A finished shape whose attach the budget deferred: a completed task cannot be polled twice.
    built: Option<Collider>,
    layers: Option<CollisionLayers>,
}

impl PendingCollider {
    /// `layers` is `None` for the default layer both audiences see.
    pub fn new(task: Task<Collider>, layers: Option<CollisionLayers>) -> Self {
        Self {
            task: Some(task),
            built: None,
            layers,
        }
    }

    #[cfg(test)]
    fn ready(collider: Collider) -> Self {
        Self {
            task: None,
            built: Some(collider),
            layers: None,
        }
    }
}

/// Attaches built colliders, at most [`ATTACH_BUDGET`] worth per frame. Exclusive so the deadline
/// can bound the inserts themselves.
pub(super) fn finish_colliders(
    world: &mut World,
    state: &mut SystemState<Query<'static, 'static, (Entity, &mut PendingCollider)>>,
) {
    const READY_SCAN_CAP: usize = 2048;
    let mut ready: Vec<Entity> = Vec::new();
    for (entity, mut pc) in &mut state.get_mut(world) {
        if let Some(task) = pc.task.as_mut()
            && let Some(collider) = block_on(future::poll_once(task))
        {
            pc.task = None;
            pc.built = Some(collider);
        }
        if pc.built.is_some() && ready.len() < READY_SCAN_CAP {
            ready.push(entity);
        }
    }
    let budget = world
        .get_resource::<AttachBudget>()
        .map_or(ATTACH_BUDGET, |b| b.0);
    let deadline = Instant::now() + budget;
    for entity in ready {
        let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
            continue;
        };
        let Some(mut pc) = entity_mut.take::<PendingCollider>() else {
            continue;
        };
        let Some(collider) = pc.built.take() else {
            continue;
        };
        entity_mut.insert((collider, RigidBody::Static));
        if let Some(layers) = pc.layers {
            entity_mut.insert(layers);
        }
        if Instant::now() >= deadline {
            break;
        }
    }
}

/// Builds a static trimesh on the async compute pool.
pub fn build_collider_task(verts: Vec<Vec3>, tris: Vec<[u32; 3]>) -> Task<Collider> {
    AsyncComputeTaskPool::get().spawn(async move { Collider::trimesh(verts, tris) })
}

/// A model's collision hull at its placement, in world space, as it is drawn. `None` for a model
/// with no hull.
pub fn placement_collider_data(
    hull: Option<&CollisionMesh>,
    transform: &Transform,
) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let hull = hull?;
    if hull.indices.len() < 3 {
        return None;
    }
    let verts = hull
        .positions
        .iter()
        .map(|p| transform.transform_point(wow_to_bevy(*p)))
        .collect();
    let tris = hull
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    Some((verts, tris))
}

/// One trimesh for a whole terrain tile, from the same chunks it is drawn from. `None` when holes
/// leave no triangle.
pub fn terrain_collider_data(chunks: &[ChunkMesh]) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut verts: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for chunk in chunks {
        let base = verts.len() as u32;
        verts.extend(chunk.positions.iter().map(|p| wow_to_bevy(*p)));
        tris.extend(
            chunk
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| [base + c[0], base + c[1], base + c[2]]),
        );
    }
    (!tris.is_empty()).then_some((verts, tris))
}

/// How far an impassable chunk's fence rises above the chunk's lowest ground, in yards.
const FENCE_REACH: f32 = 32000.0;

/// The fences of a tile's impassable chunks: all four sides of every flagged chunk, whatever its
/// neighbours, standing on the chunk's lowest ground and rising, wound outward. The one-sided law
/// then blocks a body walking in and lets one inside walk out; nothing below the floor is fenced.
pub fn impassable_wall_data(chunks: &[ChunkMesh]) -> Option<(Vec<Vec3>, Vec<[u32; 3]>)> {
    // The outer grid's corners, stride 17, each side walked so its normal points out of the chunk.
    const SIDES: [[usize; 2]; 4] = [[8, 0], [136, 144], [0, 136], [144, 8]];
    let mut verts: Vec<Vec3> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for chunk in chunks.iter().filter(|c| c.impassable) {
        let Some(floor) = chunk.positions.iter().map(|p| p[2]).reduce(f32::min) else {
            continue;
        };
        for [from, to] in SIDES {
            let (Some(a), Some(b)) = (chunk.positions.get(from), chunk.positions.get(to)) else {
                continue;
            };
            let base = verts.len() as u32;
            for (p, z) in [
                (a, floor),
                (b, floor),
                (b, floor + FENCE_REACH),
                (a, floor + FENCE_REACH),
            ] {
                verts.push(wow_to_bevy([p[0], p[1], z]));
            }
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
        }
    }
    (!tris.is_empty()).then_some((verts, tris))
}

#[cfg(test)]
mod tests;
