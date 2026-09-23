//! One-sided movement collision. The client discards a face approached from its back before it
//! computes any distance. A shape cast reports only its first hit, and when that is a backface the
//! right answer is the next front face behind it, so the gate runs where candidates are
//! enumerated: this re-runs avian's move-and-slide over its public pieces, walking each trimesh's
//! tree itself and testing every triangle's winding before it may block.
//!
//! There is no depenetration pass either: the client's resolver only sweeps, and nothing moves a
//! body that asked to go nowhere.

use core::time::Duration;

use avian3d::character_controller::move_and_slide::{
    MoveAndSlide, MoveAndSlideConfig, MoveAndSlideHitData, MoveAndSlideHitResponse,
    MoveAndSlideOutput, MoveHitData,
};
use avian3d::parry::bounding_volume::Aabb as ParryAabb;
use avian3d::parry::math::Pose3;
use avian3d::parry::query::{
    Ray, RayCast, ShapeCastOptions, ShapeCastStatus, cast_shapes, contact,
};
use avian3d::parry::shape::{TriMesh, Triangle};
use avian3d::prelude::*;
use bevy::prelude::*;

/// The client's facing gate: a face may block iff `n·dir ≤ FACING_EPS`, which is −1e-5.
const FACING_EPS: f32 = f32::from_bits(0xb727_c5ac);

/// avian's own floor on `n·dir` in its skin-width pull-back.
const DOT_EPSILON: f32 = 0.005;

/// A face every vertex of which lies further than this behind the body's leading extent along the
/// motion has been passed, and is not a hit. Measured against the vertices, not the penetration
/// depth, so a sloped floor the feet sit a little under still holds them up.
const BACKFACE_BAND: f32 = 1.0 / 36.0;

/// One-sided [`MoveAndSlide::cast_move`]: sweeps `shape` (axis-aligned, as every mover capsule
/// is) along `movement`, stopping `skin_width` short of the first face whose authored winding
/// opposes the motion. Backfaces are passed through to whatever front face lies beyond.
pub(super) fn cast_move(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    from: Vec3,
    movement: Vec3,
    skin_width: f32,
    filter: &SpatialQueryFilter,
) -> Option<MoveHitData> {
    let (dir, len) = Dir3::new_and_length(movement).unwrap_or((Dir3::X, 0.0));
    let max_time = len + skin_width;
    let a0 = shape.aabb(from, Quat::IDENTITY);
    let a1 = shape.aabb(from + movement, Quat::IDENTITY);
    let swept = ColliderAabb {
        min: a0.min.min(a1.min) - Vec3::splat(skin_width),
        max: a0.max.max(a1.max) + Vec3::splat(skin_width),
    };

    let mut best: Option<MoveHitData> = None;
    let mut best_time = max_time;
    for entity in ms.spatial_query.aabb_intersections_with_aabb(swept) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }
        let options = ShapeCastOptions {
            max_time_of_impact: best_time,
            target_distance: 0.0,
            stop_at_penetration: false,
            compute_impact_geometry_on_penetration: true,
        };
        let hit = if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            trimesh_hit(
                trimesh,
                entity,
                (pos.0, rot.0),
                shape,
                (from, dir),
                swept,
                options,
            )
        } else {
            // A convex collider has no reachable backface: avian's whole-shape sweep.
            cast_shapes(
                &Pose3::from_parts(pos.0, rot.0),
                Vec3::ZERO,
                collider.shape_scaled().as_ref(),
                &Pose3::from_parts(from, Quat::IDENTITY),
                *dir,
                shape.shape_scaled().as_ref(),
                options,
            )
            .ok()
            .flatten()
            .map(|hit| MoveHitData {
                entity,
                distance: 0.0,
                point1: pos.0 + rot.0 * hit.witness1,
                point2: from + hit.witness2 + *dir * hit.time_of_impact,
                normal1: rot.0 * hit.normal1,
                normal2: hit.normal2,
                collision_distance: hit.time_of_impact,
            })
        };
        if let Some(hit) = hit
            && hit.collision_distance < best_time
        {
            best_time = hit.collision_distance;
            best = Some(hit);
        }
    }

    best.map(|mut hit| {
        hit.distance = if max_time == 0.0 {
            0.0
        } else {
            let dot = dir.dot(-hit.normal1).max(DOT_EPSILON);
            (hit.collision_distance - skin_width / dot).max(0.0)
        };
        hit
    })
}

/// The nearest front face of a placed trimesh that `shape`, swept from `from` along `dir`, meets
/// sooner than the options' limit.
fn trimesh_hit(
    trimesh: &TriMesh,
    entity: Entity,
    (pos, rot): (Vec3, Quat),
    shape: &Collider,
    (from, dir): (Vec3, Dir3),
    swept: ColliderAabb,
    options: ShapeCastOptions,
) -> Option<MoveHitData> {
    // In the trimesh's frame; a proper rotation keeps the sign of every `n·dir`.
    let inv_rot = rot.inverse();
    let local_dir = inv_rot * *dir;
    let local_pose = Pose3::from_parts(inv_rot * (from - pos), inv_rot);
    let mut best = None;
    let mut best_time = options.max_time_of_impact;
    for tri_id in trimesh
        .bvh()
        .intersect_aabb(&aabb_to_local(swept, pos, inv_rot))
    {
        let tri = trimesh.triangle(tri_id);
        let Some(n) = tri.normal() else {
            continue;
        };
        if n.dot(local_dir) > FACING_EPS {
            continue;
        }
        let options = ShapeCastOptions {
            max_time_of_impact: best_time,
            ..options
        };
        let Ok(Some(hit)) = cast_shapes(
            &Pose3::IDENTITY,
            Vec3::ZERO,
            &tri,
            &local_pose,
            local_dir,
            shape.shape_scaled().as_ref(),
            options,
        ) else {
            continue;
        };
        if hit.status == ShapeCastStatus::PenetratingOrWithinTargetDist
            && behind_the_band(&tri, &local_pose, shape, local_dir)
        {
            continue;
        }
        if hit.time_of_impact < best_time {
            best_time = hit.time_of_impact;
            best = Some(MoveHitData {
                entity,
                distance: 0.0,
                point1: pos + rot * hit.witness1,
                point2: pos + rot * (hit.witness2 + local_dir * hit.time_of_impact),
                normal1: rot * hit.normal1,
                normal2: rot * hit.normal2,
                collision_distance: hit.time_of_impact,
            });
        }
    }
    best
}

/// One-sided [`SpatialQuery::cast_ray`].
pub(super) fn cast_ray(
    ms: &MoveAndSlide<'_, '_>,
    origin: Vec3,
    dir: Dir3,
    max_distance: f32,
    filter: &SpatialQueryFilter,
) -> Option<RayHitData> {
    let end = origin + *dir * max_distance;
    let swept = ColliderAabb {
        min: origin.min(end),
        max: origin.max(end),
    };
    let mut best: Option<RayHitData> = None;
    let mut best_time = max_distance;
    for entity in ms.spatial_query.aabb_intersections_with_aabb(swept) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }
        let inv_rot = rot.0.inverse();
        let local_ray = Ray::new(inv_rot * (origin - pos.0), inv_rot * *dir);
        let hit = if let Some(trimesh) = collider.shape_scaled().as_trimesh() {
            let mut nearest = None;
            for tri_id in trimesh
                .bvh()
                .intersect_aabb(&aabb_to_local(swept, pos.0, inv_rot))
            {
                let tri = trimesh.triangle(tri_id);
                let Some(n) = tri.normal() else {
                    continue;
                };
                if n.dot(local_ray.dir) > FACING_EPS {
                    continue;
                }
                if let Some(hit) = tri.cast_local_ray_and_get_normal(&local_ray, best_time, true)
                    && hit.time_of_impact < best_time
                {
                    best_time = hit.time_of_impact;
                    nearest = Some(hit);
                }
            }
            nearest
        } else {
            collider
                .shape_scaled()
                .cast_local_ray_and_get_normal(&local_ray, best_time, true)
                .filter(|hit| hit.time_of_impact < best_time)
        };
        if let Some(hit) = hit {
            best_time = hit.time_of_impact;
            best = Some(RayHitData {
                entity,
                distance: hit.time_of_impact,
                normal: rot.0 * hit.normal,
            });
        }
    }
    best
}

/// One-sided [`MoveAndSlide::move_and_slide`]: avian's sweep, plane collection and velocity
/// projection over the gated candidate set, without its opening and closing depenetration. The
/// `on_hit` contract is avian's.
#[allow(clippy::too_many_arguments)]
pub(super) fn move_and_slide(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    shape_position: Vec3,
    mut velocity: Vec3,
    delta_time: Duration,
    config: &MoveAndSlideConfig,
    filter: &SpatialQueryFilter,
    mut on_hit: impl FnMut(MoveAndSlideHitData<'_>) -> MoveAndSlideHitResponse,
) -> MoveAndSlideOutput {
    const MIN_DISTANCE: f32 = 1e-4;
    let mut position = shape_position;
    let mut time_left = delta_time.as_secs_f32();
    let skin_width = ms.length_unit.0 * config.skin_width;

    for _ in 0..config.move_and_slide_iterations {
        let sweep = time_left * velocity;
        let Ok((vel_dir, distance)) = Dir3::new_and_length(sweep) else {
            break;
        };
        if distance < MIN_DISTANCE {
            break;
        }
        let Some(sweep_hit) = cast_move(ms, shape, position, sweep, skin_width, filter) else {
            position += sweep;
            break;
        };
        time_left -= time_left * (sweep_hit.distance / distance);
        position += *vel_dir * sweep_hit.distance;

        let mut planes: Vec<Dir3> = config.planes.clone();
        let mut first_normal = Dir3::new_unchecked(sweep_hit.normal1);
        let response = on_hit(MoveAndSlideHitData {
            entity: sweep_hit.entity,
            point: sweep_hit.point2,
            normal: &mut first_normal,
            collision_distance: sweep_hit.collision_distance,
            distance: sweep_hit.distance,
            position: &mut position,
            velocity: &mut velocity,
        });
        match response {
            MoveAndSlideHitResponse::Accept => planes.push(first_normal),
            MoveAndSlideHitResponse::Abort => break,
            MoveAndSlideHitResponse::Ignore => {}
        }

        // avian's plane collection, per gated triangle, with the velocity as the motion.
        let mut aborted = false;
        for_each_contact(
            ms,
            shape,
            position,
            skin_width * 2.0,
            filter,
            velocity,
            |entity, point, normal| {
                let mut normal = Dir3::new_unchecked(normal);
                for existing in &mut planes {
                    if normal.dot(**existing) >= config.plane_similarity_dot_threshold {
                        if normal.dot(velocity) < existing.dot(velocity) {
                            *existing = normal;
                        }
                        return true;
                    }
                }
                if planes.len() >= config.max_planes {
                    return false;
                }
                match on_hit(MoveAndSlideHitData {
                    entity,
                    point,
                    normal: &mut normal,
                    collision_distance: sweep_hit.collision_distance,
                    distance: sweep_hit.distance,
                    position: &mut position,
                    velocity: &mut velocity,
                }) {
                    MoveAndSlideHitResponse::Accept => {
                        planes.push(normal);
                        true
                    }
                    MoveAndSlideHitResponse::Ignore => true,
                    MoveAndSlideHitResponse::Abort => {
                        aborted = true;
                        false
                    }
                }
            },
        );
        velocity = MoveAndSlide::project_velocity(velocity, &planes);
        if aborted {
            break;
        }
    }

    MoveAndSlideOutput {
        position,
        projected_velocity: velocity,
    }
}

/// Visits every contact of `shape` within `prediction` whose face opposes `motion` and has the
/// mover on its front side; behind a face its plane does not exist for movement. The callback
/// gets the world contact point and the normal toward the mover, and returns `false` to stop.
fn for_each_contact(
    ms: &MoveAndSlide<'_, '_>,
    shape: &Collider,
    position: Vec3,
    prediction: f32,
    filter: &SpatialQueryFilter,
    motion: Vec3,
    mut callback: impl FnMut(Entity, Vec3, Vec3) -> bool,
) {
    let Ok(motion_dir) = Dir3::new(motion) else {
        return;
    };
    let a = shape.aabb(position, Quat::IDENTITY);
    let grown = ColliderAabb {
        min: a.min - Vec3::splat(prediction),
        max: a.max + Vec3::splat(prediction),
    };
    for entity in ms.spatial_query.aabb_intersections_with_aabb(grown) {
        let Ok((collider, pos, rot, layers)) = ms.colliders.get(entity) else {
            continue;
        };
        if !filter.test(entity, layers.copied().unwrap_or_default()) {
            continue;
        }
        let Some(trimesh) = collider.shape_scaled().as_trimesh() else {
            let Ok(Some(c)) = contact(
                &Pose3::from_parts(pos.0, rot.0),
                collider.shape_scaled().as_ref(),
                &Pose3::from_parts(position, Quat::IDENTITY),
                shape.shape_scaled().as_ref(),
                prediction,
            ) else {
                continue;
            };
            if !callback(entity, c.point1, c.normal1) {
                return;
            }
            continue;
        };
        let inv_rot = rot.0.inverse();
        let local_pose = Pose3::from_parts(inv_rot * (position - pos.0), inv_rot);
        let local_dir = inv_rot * *motion_dir;
        for tri_id in trimesh
            .bvh()
            .intersect_aabb(&aabb_to_local(grown, pos.0, inv_rot))
        {
            let tri = trimesh.triangle(tri_id);
            let Some(n) = tri.normal() else {
                continue;
            };
            if n.dot(local_dir) > FACING_EPS {
                continue;
            }
            let Ok(Some(c)) = contact(
                &Pose3::IDENTITY,
                &tri,
                &local_pose,
                shape.shape_scaled().as_ref(),
                prediction,
            ) else {
                continue;
            };
            if c.normal1.dot(n) <= 0.0 {
                continue;
            }
            if c.dist < 0.0 && behind_the_band(&tri, &local_pose, shape, local_dir) {
                continue;
            }
            if !callback(entity, pos.0 + rot.0 * c.point1, rot.0 * c.normal1) {
                return;
            }
        }
    }
}

/// Has the mover already passed this triangle by more than [`BACKFACE_BAND`]? Every vertex is
/// measured against the shape's support point along `dir`. A shape with no support map never is.
fn behind_the_band(tri: &Triangle, shape_pose: &Pose3, shape: &Collider, dir: Vec3) -> bool {
    let Some(support) = shape.shape_scaled().as_support_map() else {
        return false;
    };
    let lead = support.support_point(shape_pose, dir).dot(dir);
    let ahead = [tri.a, tri.b, tri.c]
        .into_iter()
        .map(|v| v.dot(dir) - lead)
        .fold(f32::MIN, f32::max);
    ahead < -BACKFACE_BAND
}

/// The local-frame box enclosing a world box under `local = inv_rot · (world − pos)`.
fn aabb_to_local(aabb: ColliderAabb, pos: Vec3, inv_rot: Quat) -> ParryAabb {
    let (mut mins, mut maxs) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { aabb.min.x } else { aabb.max.x },
            if i & 2 == 0 { aabb.min.y } else { aabb.max.y },
            if i & 4 == 0 { aabb.min.z } else { aabb.max.z },
        );
        let local = inv_rot * (corner - pos);
        mins = mins.min(local);
        maxs = maxs.max(local);
    }
    ParryAabb::new(mins, maxs)
}

#[cfg(test)]
mod tests;
