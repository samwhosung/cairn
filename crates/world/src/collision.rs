//! The world's collision: the streamed terrain, building and doodad colliders, and the queries a
//! walking body and the follow camera ask of them.
//!
//! The body and the camera collide with different WMO faces, as the client's two gathers do: the
//! walking one drops detail faces, the camera's drops the faces flagged camera-transparent. One
//! trimesh cannot be filtered per face, so each building bakes two colliders on two layers.

mod assets;
mod colliders;
mod edits;
mod liquid;
mod one_sided;
mod stream;
#[cfg(test)]
mod tests;
mod weld;

use std::time::Duration;

use avian3d::character_controller::move_and_slide::{
    MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitData, MoveAndSlideHitResponse,
    MoveAndSlideOutput, MoveHitData,
};
use avian3d::dynamics::solver::joint_graph::JointGraphPlugin;
use avian3d::prelude::*;
use bevy::app::PluginGroupBuilder;
use bevy::asset::AssetApp;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

pub use assets::{M2Hull, TileCollision, WmoHull};
pub use colliders::{
    ATTACH_BUDGET, AttachBudget, PendingCollider, build_collider_task, impassable_wall_data,
    placement_collider_data, terrain_collider_data,
};
pub use liquid::{Liquids, NearestLiquid, SwimSurface};
pub use stream::CollisionResidency;

/// The two audiences and the liquid surfaces. Terrain and doodads carry no explicit layer, so
/// both audiences see them.
#[derive(PhysicsLayer, Default, Clone, Copy)]
pub enum CollisionLayer {
    #[default]
    Default,
    /// WMO faces only the walking body collides with.
    Walk,
    /// WMO faces only the camera collides with.
    Camera,
    /// Liquid surfaces: nothing sees them unless a query asks by name.
    Liquid,
}

/// The sphere the camera boom sweeps against solid geometry, in yards: the margin that keeps the
/// near plane out of a wall. The client's own trace is a bare ray.
pub const CAMERA_PROBE_RADIUS: f32 = 0.3;

/// On a static trimesh collider, baked in world space, whose triangles take ground decals: a
/// tile's terrain and the faces a walking body meets in a building, never a doodad.
#[derive(Component)]
pub struct GroundDecalSurface;

pub(crate) fn walk_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Walk, LayerMask::ALL)
}

pub(crate) fn camera_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Camera, LayerMask::ALL)
}

pub(crate) fn liquid_layers() -> CollisionLayers {
    CollisionLayers::new(CollisionLayer::Liquid, LayerMask::ALL)
}

/// Streams colliders for the tiles around the [`WorldCamera`](crate::WorldCamera) and answers
/// collision and liquid queries. Streaming needs the [`Install`](crate::Install) source and the
/// [`CurrentMap`](crate::CurrentMap); without a map, the queries see only colliders spawned in code.
pub struct CollisionPlugin;

impl Plugin for CollisionPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(physics_plugins(FixedPostUpdate))
            .insert_resource(SubstepCount(1))
            .insert_resource(avian3d::physics_transform::PhysicsTransformConfig {
                propagate_before_physics: false,
                ..default()
            })
            // As a task, the trees' upkeep would wait behind every collider build on the shared
            // pool, and nothing is left in the step to overlap it.
            .insert_resource(ColliderTreeOptimization {
                use_async_tasks: false,
                ..default()
            })
            .init_asset::<TileCollision>()
            .init_asset::<M2Hull>()
            .init_asset::<WmoHull>()
            .register_asset_loader(assets::TileCollisionLoader)
            .register_asset_loader(assets::M2HullLoader)
            .register_asset_loader(assets::WmoHullLoader)
            .init_resource::<stream::CollisionStreamer>()
            .init_resource::<CollisionResidency>()
            .init_resource::<crate::PlacementEdits>()
            .init_resource::<liquid::SwimIndex>()
            .add_systems(
                Update,
                (
                    colliders::finish_colliders,
                    stream::stream_collision.run_if(resource_exists::<crate::CurrentMap>),
                    edits::follow_edits,
                    stream::spawn_placement_colliders,
                    liquid::maintain_water_index,
                    stream::publish_residency,
                )
                    .chain()
                    .in_set(crate::WorldSystems),
            );
    }
}

/// avian's plugins with everything but collider storage, the collider trees and spatial queries
/// taken out: nothing here has mass, velocity, contacts or joints.
pub fn physics_plugins(schedule: impl ScheduleLabel) -> PluginGroupBuilder {
    PhysicsPlugins::new(schedule)
        .build()
        .disable::<BvhBroadPhasePlugin>()
        .disable::<ForcePlugin>()
        .disable::<MassPropertyPlugin>()
        .disable::<NarrowPhasePlugin<Collider>>()
        .disable::<JointPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
        .disable::<SolverBodyPlugin>()
        .disable::<IntegratorPlugin>()
        .disable::<SolverPlugin>()
        .disable::<CcdPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>()
        .disable::<JointGraphPlugin<FixedJoint>>()
        .disable::<JointGraphPlugin<RevoluteJoint>>()
        .disable::<JointGraphPlugin<PrismaticJoint>>()
        .disable::<JointGraphPlugin<DistanceJoint>>()
        .disable::<JointGraphPlugin<SphericalJoint>>()
}

/// Traces a body or the camera through the world. The body's sweeps are one-sided: a face
/// approached from its back is not a candidate at all, as in the client's movement collision.
#[derive(SystemParam)]
pub struct WorldCollision<'w, 's> {
    ms: MoveAndSlide<'w, 's>,
}

impl WorldCollision<'_, '_> {
    /// What a walking body collides with: the default layer and the walk-only WMO faces.
    pub fn body_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Walk.to_bits(),
        ))
    }

    fn camera_filter() -> SpatialQueryFilter {
        SpatialQueryFilter::from_mask(LayerMask(
            CollisionLayer::Default.to_bits() | CollisionLayer::Camera.to_bits(),
        ))
    }

    /// The camera's waterline leg: a two-sided ray against the liquid surfaces, because the
    /// clearance the camera keeps above the water is sized for a ray, not the probe sphere.
    fn waterline_ray(&self, from: Vec3, movement: Vec3) -> Option<f32> {
        let dir = Dir3::new(movement).ok()?;
        self.ms
            .spatial_query
            .cast_ray(
                from,
                dir,
                movement.length(),
                true,
                &SpatialQueryFilter::from_mask(LayerMask(CollisionLayer::Liquid.to_bits())),
            )
            .map(|h| h.distance)
    }

    /// Sweeps `shape` along `movement` against the body's world, stopping `skin_width` short.
    pub fn cast_body(
        &self,
        shape: &Collider,
        from: Vec3,
        movement: Vec3,
        skin_width: f32,
    ) -> Option<MoveHitData> {
        one_sided::cast_move(
            &self.ms,
            shape,
            from,
            movement,
            skin_width,
            &Self::body_filter(),
        )
    }

    /// How far along `movement` the camera boom is free, or `None` when nothing is in the way.
    /// Solid geometry is swept with the probe sphere from either side; with `liquid` the
    /// waterline stops it too, and the nearer of the two wins.
    pub fn cast_camera(&self, from: Vec3, movement: Vec3, liquid: bool) -> Option<f32> {
        let solid = self
            .ms
            .cast_move(
                &Collider::sphere(CAMERA_PROBE_RADIUS),
                from,
                Quat::IDENTITY,
                movement,
                0.0,
                &Self::camera_filter(),
            )
            .map(|h| h.distance);
        let water = liquid.then(|| self.waterline_ray(from, movement)).flatten();
        match (solid, water) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// A one-sided ray against the body's world.
    pub fn ray_body(&self, origin: Vec3, dir: Dir3, max_distance: f32) -> Option<RayHitData> {
        one_sided::cast_ray(&self.ms, origin, dir, max_distance, &Self::body_filter())
    }

    /// Moves `shape` by `velocity` for `delta_time`, sliding along what it meets.
    pub fn slide_body(
        &self,
        shape: &Collider,
        shape_position: Vec3,
        velocity: Vec3,
        delta_time: Duration,
        config: &MoveAndSlideConfig,
        on_hit: impl FnMut(MoveAndSlideHitData<'_>) -> MoveAndSlideHitResponse,
    ) -> MoveAndSlideOutput {
        one_sided::move_and_slide(
            &self.ms,
            shape,
            shape_position,
            velocity,
            delta_time,
            config,
            &Self::body_filter(),
            on_hit,
        )
    }
}
