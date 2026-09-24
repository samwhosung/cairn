use std::sync::Arc;

use bevy::camera::Projection;
use bevy::camera::primitives::{Aabb, Frustum};
use bevy::math::{Affine3A, Vec4};
use bevy::prelude::*;
use model::WmoPortalInfo;

use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::ground::terrain_wow_z_under;
use crate::room::{CameraRoom, RoomFog, select_room_fog};
use crate::stream::Streamer;
use crate::view::WorldCamera;
use crate::wmo::{Triangle, WmoGroupNav, WmoModel};

const EXTERIOR: u32 = 0x8;
/// An indoor group lit as outdoors.
const EXTERIOR_LIT: u32 = 0x40;
/// A group the client draws in a pass of its own, against the whole frustum.
const CALLBACK_PASS: u32 = 0x10000;
const ON_PLANE_EPS: f32 = 0.01;
const W_CLAMP_BAND: f32 = 0.001;
const W_CLAMP_SUB: f32 = 1.0e-5;
const RECT_EPS: f32 = 0.001;
const PORTAL_NEAR_PARALLEL: f32 = 1.0e-4;
const PORTAL_PLANE_SNAP: f32 = 0.1;
const NEAREST_TIE_EPS: f32 = 1.0e-4;
const MAX_FLOOR_DROP: f32 = 1760.0;
const DEPTH_CAP: u32 = 10;
const MAX_ITERS: u32 = 1 << 16;

const FULL_SCREEN: Rect = Rect {
    min: Vec2::NEG_ONE,
    max: Vec2::ONE,
};

#[derive(Component)]
pub struct WmoPortalInstance {
    pub(crate) handle: Handle<WmoModel>,
    pub(crate) world_from_local: Affine3A,
    /// The placement's `WMOAreaTable` name set.
    pub(crate) name_set: u16,
    pub(crate) visible: Vec<bool>,
    pub(crate) interior_fog: Vec<bool>,
}

impl WmoPortalInstance {
    pub(crate) fn new(
        handle: Handle<WmoModel>,
        transform: &Transform,
        groups: usize,
        name_set: u16,
    ) -> Self {
        Self {
            handle,
            world_from_local: transform.compute_affine(),
            name_set,
            visible: vec![true; groups],
            interior_fog: vec![false; groups],
        }
    }
}

#[derive(Component, Clone)]
pub struct WmoGroupVis {
    pub(crate) instance: Entity,
    pub(crate) groups: Arc<[u16]>,
}

impl WmoGroupVis {
    pub(crate) fn drawn_by(&self, inst: &WmoPortalInstance) -> bool {
        self.groups
            .iter()
            .any(|&g| inst.visible.get(g as usize).copied().unwrap_or(true))
    }

    pub(crate) fn interior_fogged_by(&self, inst: &WmoPortalInstance) -> bool {
        self.groups
            .iter()
            .any(|&g| inst.interior_fog.get(g as usize).copied().unwrap_or(false))
    }
}

pub(crate) fn room_admits(room: Option<&WmoGroupVis>, inst: Option<&WmoPortalInstance>) -> bool {
    match (room, inst) {
        (None, _) => true,
        (Some(r), Some(inst)) => r.drawn_by(inst),
        (Some(_), None) => false,
    }
}

pub(crate) fn compute_wmo_pvs(
    wmos: Res<'_, Assets<WmoModel>>,
    camera: Query<'_, '_, (&GlobalTransform, &Projection), With<WorldCamera>>,
    streamer: Res<'_, Streamer>,
    adts: Res<'_, Assets<AdtTile>>,
    mut instances: Query<'_, '_, &mut WmoPortalInstance>,
    mut room: ResMut<'_, CameraRoom>,
) {
    let Ok((cam, projection)) = camera.single() else {
        return;
    };
    let clip_from_world = projection.get_clip_from_view() * cam.to_matrix().inverse();
    let eye_world = cam.translation();
    let terrain = terrain_wow_z_under(&streamer, &adts, eye_world);
    let mut found = CameraRoom::default();
    for mut inst in &mut instances {
        let Some(model) = wmos.get(&inst.handle) else {
            continue;
        };
        let groups = model.group_nav.len();
        if model.portal_refs.is_empty() || model.portal_infos.is_empty() {
            let (visible, fog) = (vec![true; groups], vec![false; groups]);
            if inst.visible != visible || inst.interior_fog != fog {
                inst.visible = visible;
                inst.interior_fog = fog;
            }
            continue;
        }
        let world_from_local = inst.world_from_local;
        let local_from_world = world_from_local.inverse();
        let eye_local = bevy_to_wow(local_from_world.transform_point3(eye_world));
        let terrain_local = terrain.map(|z| terrain_z_local(&local_from_world, eye_world, z));
        let pvs = compute_pvs(
            model,
            eye_local,
            terrain_local,
            &clip_from_world,
            &world_from_local,
        );
        if inst.visible != pvs.visible {
            inst.visible = pvs.visible;
        }
        if inst.interior_fog != pvs.interior_fog {
            inst.interior_fog = pvs.interior_fog;
        }
        if !found.indoors && pvs.seeds.indoors(&model.group_nav) {
            found = CameraRoom {
                indoors: true,
                fog: room_fog(model, pvs.seeds, eye_local),
            };
        }
    }
    if *room != found {
        *room = found;
    }
}

fn truly_interior(nav: &WmoGroupNav) -> bool {
    nav.flags & (EXTERIOR | EXTERIOR_LIT) == 0
}

fn room_fog(model: &WmoModel, seeds: DownRaySeeds, eye_local: [f32; 3]) -> Option<RoomFog> {
    [seeds.in_group, seeds.across]
        .into_iter()
        .flatten()
        .find_map(|g| model.group_nav.get(g).filter(|n| truly_interior(n)))
        .and_then(|n| select_room_fog(&model.fogs, n.fog_indices, eye_local))
}

pub(crate) fn terrain_z_local(
    local_from_world: &Affine3A,
    eye_world: Vec3,
    terrain_wow_z: f32,
) -> f32 {
    let eye_wow = bevy_to_wow(eye_world);
    let surface = wow_to_bevy([eye_wow[0], eye_wow[1], terrain_wow_z]);
    bevy_to_wow(local_from_world.transform_point3(surface))[2]
}

struct GroupPvs {
    visible: Vec<bool>,
    interior_fog: Vec<bool>,
    seeds: DownRaySeeds,
}

struct Step {
    group: usize,
    came_from: Option<usize>,
    rect: Rect,
    depth: u32,
    interior_chain: bool,
}

impl Step {
    fn seed(group: usize, rect: Rect, interior_chain: bool) -> Self {
        Self {
            group,
            came_from: None,
            rect,
            depth: 0,
            interior_chain,
        }
    }
}

struct Flood<'a> {
    model: &'a WmoModel,
    eye_local: [f32; 3],
    clip_from_world: &'a Mat4,
    world_from_local: &'a Affine3A,
    visible: Vec<bool>,
    interior_fog: Vec<bool>,
    windows: Vec<Rect>,
    portal_pushed: Vec<bool>,
    iters: u32,
}

impl Flood<'_> {
    fn walk(&mut self, stack: &mut Vec<Step>, record_windows: bool) {
        let nav = &self.model.group_nav;
        while let Some(step) = stack.pop() {
            self.iters += 1;
            let g = step.group;
            if self.iters > MAX_ITERS || g >= nav.len() {
                continue;
            }
            self.visible[g] = true;
            let chain_on = step.interior_chain && truly_interior(&nav[g]);
            self.interior_fog[g] |= chain_on;
            if step.depth >= DEPTH_CAP {
                continue;
            }
            let start = nav[g].ref_start as usize;
            let end = (start + nav[g].ref_count as usize).min(self.model.portal_refs.len());
            for r in &self.model.portal_refs[start.min(end)..end] {
                let neighbour = r.group as usize;
                if r.group == u16::MAX || Some(neighbour) == step.came_from {
                    continue;
                }
                let Some(info) = self.model.portal_infos.get(r.portal as usize) else {
                    continue;
                };
                let e = self.eye_local;
                let mut d = info.plane[0] * e[0]
                    + info.plane[1] * e[1]
                    + info.plane[2] * e[2]
                    + info.plane[3];
                if r.side < 0 {
                    d = -d;
                }
                if d < 0.0 {
                    continue;
                }
                let prect = if eye_on_portal(&self.model.portal_vertices, info, e) {
                    FULL_SCREEN
                } else {
                    let Some(p) = portal_screen_rect(
                        &self.model.portal_vertices,
                        info,
                        self.clip_from_world,
                        self.world_from_local,
                    ) else {
                        continue;
                    };
                    p
                };
                let Some(inter) = intersect_rect(step.rect, prect) else {
                    continue;
                };
                if record_windows
                    && nav.get(neighbour).is_some_and(|n| n.flags & EXTERIOR != 0)
                    && let Some(stamp) = self.portal_pushed.get_mut(r.portal as usize)
                    && !*stamp
                {
                    *stamp = true;
                    self.windows.push(inter);
                }
                stack.push(Step {
                    group: neighbour,
                    came_from: Some(g),
                    rect: inter,
                    depth: step.depth + 1,
                    interior_chain: chain_on,
                });
            }
        }
    }

    fn walk_windows(&mut self) {
        for rect in std::mem::take(&mut self.windows) {
            let frustum = window_frustum(rect, self.clip_from_world);
            let mut roots: Vec<Step> = self
                .model
                .group_nav
                .iter()
                .enumerate()
                .filter(|(_, g)| {
                    g.flags & (EXTERIOR | CALLBACK_PASS) == EXTERIOR
                        && frustum.intersects_obb(&group_aabb(g), self.world_from_local, true, true)
                })
                .map(|(gi, _)| Step::seed(gi, rect, false))
                .collect();
            self.walk(&mut roots, false);
        }
    }

    fn add_callback_groups(&mut self) {
        let base = Frustum::from_clip_from_world(self.clip_from_world);
        for (gi, g) in self.model.group_nav.iter().enumerate() {
            if self.visible[gi] || g.flags & CALLBACK_PASS == 0 {
                continue;
            }
            self.visible[gi] |=
                base.intersects_obb(&group_aabb(g), self.world_from_local, true, true);
        }
    }
}

fn compute_pvs(
    model: &WmoModel,
    eye_local: [f32; 3],
    terrain_z: Option<f32>,
    clip_from_world: &Mat4,
    world_from_local: &Affine3A,
) -> GroupPvs {
    let nav = &model.group_nav;
    let mut flood = Flood {
        model,
        eye_local,
        clip_from_world,
        world_from_local,
        visible: vec![false; nav.len()],
        interior_fog: vec![false; nav.len()],
        windows: Vec::new(),
        portal_pushed: vec![false; model.portal_infos.len()],
        iters: 0,
    };
    let seeds = down_ray_seeds(model, eye_local, terrain_z);
    let mut stack = seed_steps(nav, seeds);
    flood.walk(&mut stack, seeds.indoors(nav));
    flood.walk_windows();
    flood.add_callback_groups();
    GroupPvs {
        visible: flood.visible,
        interior_fog: flood.interior_fog,
        seeds,
    }
}

fn seed_steps(nav: &[WmoGroupNav], seeds: DownRaySeeds) -> Vec<Step> {
    match seeds.in_group {
        Some(group) => std::iter::once(group)
            .chain(seeds.across)
            .map(|g| Step::seed(g, FULL_SCREEN, true))
            .collect(),
        None => nav
            .iter()
            .enumerate()
            .filter(|(_, g)| g.flags & EXTERIOR != 0)
            .map(|(gi, _)| Step::seed(gi, FULL_SCREEN, false))
            .collect(),
    }
}

fn window_frustum(rect: Rect, clip_from_world: &Mat4) -> Frustum {
    let (w, h) = (rect.width(), rect.height());
    let rect_to_ndc = Mat4::from_cols(
        Vec4::new(2.0 / w, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / h, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(
            -(rect.min.x + rect.max.x) / w,
            -(rect.min.y + rect.max.y) / h,
            0.0,
            1.0,
        ),
    );
    Frustum::from_clip_from_world(&(rect_to_ndc * *clip_from_world))
}

fn group_aabb(nav: &WmoGroupNav) -> Aabb {
    let a = wow_to_bevy(nav.bbox_min);
    let b = wow_to_bevy(nav.bbox_max);
    Aabb::from_min_max(a.min(b), a.max(b))
}

fn portal_poly<'a>(vertices: &'a [[f32; 3]], info: &WmoPortalInfo) -> Option<&'a [[f32; 3]]> {
    let start = info.start_vertex as usize;
    let verts = vertices.get(start..start + info.count as usize)?;
    (verts.len() >= 3).then_some(verts)
}

fn eye_on_portal(vertices: &[[f32; 3]], info: &WmoPortalInfo, eye: [f32; 3]) -> bool {
    let [nx, ny, nz, d] = info.plane;
    if (nx * eye[0] + ny * eye[1] + nz * eye[2] + d).abs() > ON_PLANE_EPS {
        return false;
    }
    let Some(verts) = portal_poly(vertices, info) else {
        return false;
    };
    let (u, v) = projection_axes(info.plane);
    point_in_poly_even_odd(verts.iter().map(|p| (p[u], p[v])), (eye[u], eye[v]))
}

fn point_in_poly_even_odd(pts: impl Iterator<Item = (f32, f32)> + Clone, p: (f32, f32)) -> bool {
    let mut inside = false;
    let mut prev = pts.clone().last();
    for cur in pts {
        if let Some(pr) = prev
            && (cur.1 > p.1) != (pr.1 > p.1)
            && p.0 < (pr.0 - cur.0) * (p.1 - cur.1) / (pr.1 - cur.1) + cur.0
        {
            inside = !inside;
        }
        prev = Some(cur);
    }
    inside
}

fn projection_axes(plane: [f32; 4]) -> (usize, usize) {
    let [nx, ny, nz, _] = plane;
    if nx.abs() >= ny.abs() && nx.abs() >= nz.abs() {
        (1, 2)
    } else if ny.abs() >= nz.abs() {
        (0, 2)
    } else {
        (0, 1)
    }
}

fn portal_screen_rect(
    vertices: &[[f32; 3]],
    info: &WmoPortalInfo,
    clip_from_world: &Mat4,
    world_from_local: &Affine3A,
) -> Option<Rect> {
    let verts = portal_poly(vertices, info)?;
    let clip: Vec<Vec4> = verts
        .iter()
        .map(|v| {
            let world = world_from_local.transform_point3(wow_to_bevy(*v));
            *clip_from_world * world.extend(1.0)
        })
        .collect();
    let clipped = clip_side_planes(&clip)?;
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in &clipped {
        let w = if c.w.abs() < W_CLAMP_BAND {
            W_CLAMP_SUB
        } else {
            c.w
        };
        let inv = 1.0 / w;
        let (x, y) = (c.x * inv, c.y * inv);
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    Some(Rect {
        min: Vec2::new(min_x, min_y),
        max: Vec2::new(max_x, max_y),
    })
}

/// The client's guard pyramid: the four side planes and no near plane.
fn clip_side_planes(poly: &[Vec4]) -> Option<Vec<Vec4>> {
    const PLANES: [fn(&Vec4) -> f32; 4] =
        [|v| v.w + v.x, |v| v.w - v.x, |v| v.w + v.y, |v| v.w - v.y];
    let mut cur = poly.to_vec();
    for f in PLANES {
        let n = cur.len();
        let mut out: Vec<Vec4> = Vec::with_capacity(n + 2);
        for i in 0..n {
            let (a, b) = (cur[i], cur[(i + 1) % n]);
            let (fa, fb) = (f(&a), f(&b));
            if fa >= 0.0 {
                out.push(a);
            }
            if (fa >= 0.0) != (fb >= 0.0) {
                let t = fa / (fa - fb);
                out.push(a + (b - a) * t);
            }
        }
        cur = out;
        if cur.len() < 3 {
            return None;
        }
    }
    Some(cur)
}

fn intersect_rect(a: Rect, b: Rect) -> Option<Rect> {
    let rect = Rect {
        min: a.min.max(b.min),
        max: a.max.min(b.max),
    };
    (rect.width() >= RECT_EPS && rect.height() >= RECT_EPS).then_some(rect)
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct DownRaySeeds {
    pub(crate) in_group: Option<usize>,
    across: Option<usize>,
}

impl DownRaySeeds {
    fn indoors(&self, nav: &[WmoGroupNav]) -> bool {
        self.in_group
            .and_then(|gi| nav.get(gi))
            .is_some_and(|n| n.flags & EXTERIOR == 0)
    }
}

pub(crate) fn down_ray_seeds(
    model: &WmoModel,
    eye: [f32; 3],
    terrain_z: Option<f32>,
) -> DownRaySeeds {
    let nav = &model.group_nav;
    let in_column = |g: &WmoGroupNav| {
        eye[0] >= g.bbox_min[0]
            && eye[0] <= g.bbox_max[0]
            && eye[1] >= g.bbox_min[1]
            && eye[1] <= g.bbox_max[1]
            && g.bbox_min[2] <= eye[2]
    };
    let highest_face = |tris: &[Vec<Triangle>]| {
        let mut best: (f32, Option<usize>) = (f32::NEG_INFINITY, None);
        for (gi, g) in nav.iter().enumerate() {
            if !in_column(g) {
                continue;
            }
            for tri in tris.get(gi).into_iter().flatten() {
                if let Some(z) = terrain::triangle_z_at(tri, eye[0], eye[1])
                    && z <= eye[2]
                    && z > best.0
                {
                    best = (z, Some(gi));
                }
            }
        }
        best
    };
    let (mut best_z, mut best) = highest_face(&model.group_collision_tris);
    let mut across = None;
    for (gi, g) in nav.iter().enumerate() {
        if !in_column(g) {
            continue;
        }
        let start = g.ref_start as usize;
        let end = (start + g.ref_count as usize).min(model.portal_refs.len());
        for r in &model.portal_refs[start.min(end)..end] {
            let Some(info) = model.portal_infos.get(r.portal as usize) else {
                continue;
            };
            let [nx, ny, nz, d] = info.plane;
            let z = if nz.abs() < PORTAL_NEAR_PARALLEL {
                if (nx * eye[0] + ny * eye[1] + nz * eye[2] + d).abs() > PORTAL_PLANE_SNAP {
                    continue;
                }
                eye[2]
            } else {
                let z = -(nx * eye[0] + ny * eye[1] + d) / nz;
                if z > eye[2] {
                    continue;
                }
                z
            };
            if z < best_z - NEAREST_TIE_EPS {
                continue;
            }
            let Some(verts) = portal_poly(&model.portal_vertices, info) else {
                continue;
            };
            let (u, v) = projection_axes(info.plane);
            let hit = [eye[0], eye[1], z];
            if !point_in_poly_even_odd(verts.iter().map(|q| (q[u], q[v])), (hit[u], hit[v])) {
                continue;
            }
            let d_signed = nx * eye[0] + ny * eye[1] + nz * eye[2] + d;
            let chosen = if (d_signed >= 0.0) == (r.side > 0) {
                gi
            } else {
                r.group as usize
            };
            if nav.get(chosen).is_none() {
                continue;
            }
            let other = if chosen == gi { r.group as usize } else { gi };
            best_z = z;
            best = Some(chosen);
            across = (nav.get(other).is_some() && other != chosen).then_some(other);
        }
    }
    let Some(in_group) = best else {
        if terrain_z.is_some_and(|tz| tz <= eye[2]) {
            return DownRaySeeds::default();
        }
        let (fb_z, fb) = highest_face(&model.group_camera_only_tris);
        let named = fb.filter(|&g| {
            eye[2] - fb_z <= MAX_FLOOR_DROP && nav.get(g).is_some_and(|n| n.flags & EXTERIOR == 0)
        });
        return DownRaySeeds {
            in_group: named,
            across: None,
        };
    };
    if eye[2] - best_z > MAX_FLOOR_DROP || nav.get(in_group).is_none_or(|g| g.flags & EXTERIOR != 0)
    {
        return DownRaySeeds::default();
    }
    if terrain_z.is_some_and(|tz| tz <= eye[2] && tz > best_z) {
        return DownRaySeeds::default();
    }
    DownRaySeeds {
        in_group: Some(in_group),
        across,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portal_the_eye_stands_in_is_open() {
        let verts = vec![
            [0.0, -2.0, 0.0],
            [0.0, 2.0, 0.0],
            [0.0, 2.0, 4.0],
            [0.0, -2.0, 4.0],
        ];
        let info = WmoPortalInfo {
            start_vertex: 0,
            count: 4,
            plane: [1.0, 0.0, 0.0, 0.0],
        };
        assert!(eye_on_portal(&verts, &info, [0.0, 0.0, 2.0]));
        assert!(eye_on_portal(&verts, &info, [0.005, 1.5, 3.5]));
        assert!(!eye_on_portal(&verts, &info, [0.0, 0.0, 5.0]));
        assert!(!eye_on_portal(&verts, &info, [0.05, 0.0, 2.0]));
    }

    #[test]
    fn rects_meet_only_with_area() {
        assert_eq!(
            intersect_rect(
                Rect::new(-1.0, -1.0, 0.5, 0.5),
                Rect::new(0.0, 0.0, 1.0, 1.0)
            ),
            Some(Rect::new(0.0, 0.0, 0.5, 0.5))
        );
        assert_eq!(
            intersect_rect(
                Rect::new(-1.0, -1.0, 0.0, 1.0),
                Rect::new(0.0005, -1.0, 1.0, 1.0)
            ),
            None
        );
    }

    #[test]
    fn a_polygon_behind_the_side_planes_is_gone() {
        let off = [
            Vec4::new(5.0, 0.0, 0.5, 1.0),
            Vec4::new(6.0, 0.5, 0.5, 1.0),
            Vec4::new(5.0, 1.0, 0.5, 1.0),
        ];
        assert!(clip_side_planes(&off).is_none());
        let on = off.map(|v| v - Vec4::new(5.0, 0.0, 0.0, 0.0));
        assert!(clip_side_planes(&on).is_some());
    }
}
