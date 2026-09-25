//! What a ray from the camera meets among what is drawn: the terrain's faces and the faces of the
//! models the map places, each named as the map's files name it.

use std::sync::Arc;

use bevy::camera::primitives::Aabb;
use bevy::ecs::system::SystemParam;
use bevy::math::Affine3A;
use bevy::prelude::*;
use model::{ALPHA_KEY_REF, Coverage, CoverageReader, RenderSubmesh};
use mpq::Chain;
use terrain::{CHUNK_SIZE, ChunkMesh};

use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::source::{Install, MPQ_SOURCE};
use crate::stream::Streamer;

/// What a ray met. A file is its path in the install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// The terrain of the tile's ADT: the chunk's column and row in it.
    Terrain { chunk: (u32, u32) },
    /// An M2 the map places, by its placement's unique id in the ADTs.
    Doodad { file: Arc<str>, unique_id: u32 },
    /// A group of a WMO the map places.
    Building {
        file: Arc<str>,
        unique_id: u32,
        group: u16,
    },
    /// An M2 a WMO places, by its index among the WMO's doodads; `building` is the WMO's file and
    /// `unique_id` the WMO's placement.
    Prop {
        file: Arc<str>,
        building: Arc<str>,
        unique_id: u32,
        doodad: usize,
    },
}

/// Where a ray met what is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Sighting {
    /// Bevy's axes.
    pub point: Vec3,
    pub distance: f32,
    pub seen: Seen,
    /// The ADT tile `(x, y)` of the map's `<map>_<x>_<y>.adt`: the terrain's own, or the one
    /// under the point.
    pub tile: (u32, u32),
}

/// A model batch as a ray meets it.
#[derive(Component, Clone)]
pub(crate) struct Meetable {
    /// The batch's triangles in the model's own space.
    pub(crate) geometry: Arc<RenderSubmesh>,
    pub(crate) seen: Seen,
}

/// The install path of an asset the world loads.
pub(crate) fn file_of(url: &str) -> Arc<str> {
    Arc::from(url.strip_prefix(&format!("{MPQ_SOURCE}://")).unwrap_or(url))
}

type Batch = (
    &'static Meetable,
    &'static GlobalTransform,
    &'static ViewVisibility,
    Option<&'static Aabb>,
);

/// Casts rays into what is drawn this frame.
#[derive(SystemParam)]
pub struct Sight<'w, 's> {
    batches: Query<'w, 's, Batch>,
    streamer: Res<'w, Streamer>,
    tiles: Res<'w, Assets<AdtTile>>,
    install: Res<'w, Install>,
}

impl Sight<'_, '_> {
    /// A ray from `origin` along `dir`, in Bevy's axes, out to `reach`. The terrain's nearest face
    /// is found now, and the model batches drawn this frame that the ray enters nearer than it are
    /// kept for [`Ray::first`], which can run on any thread.
    pub fn cast(&self, origin: Vec3, dir: Dir3, reach: f32) -> Ray {
        let mut terrain: Option<(f32, Seen, (u32, u32))> = None;
        let (o, d) = (
            Vec3::from(bevy_to_wow(origin)),
            Vec3::from(bevy_to_wow(*dir)),
        );
        for (tile, handle) in self.streamer.arrived() {
            for chunk in self.tiles.get(handle).into_iter().flat_map(|t| &t.chunks) {
                let limit = terrain.as_ref().map_or(reach, |t| t.0);
                if let Some(t) = chunk_hit(chunk, o, d, limit) {
                    let seen = Seen::Terrain {
                        chunk: (chunk.index_x, chunk.index_y),
                    };
                    terrain = Some((t, seen, tile));
                }
            }
        }
        let limit = terrain.as_ref().map_or(reach, |t| t.0);
        let mut batches: Vec<Candidate> = Vec::new();
        for (batch, placed, drawn, bound) in &self.batches {
            if !drawn.get() {
                continue;
            }
            let to_mesh = placed.affine().inverse();
            let (o, d) = (
                to_mesh.transform_point3(origin),
                to_mesh.transform_vector3(*dir),
            );
            let enters = match bound {
                Some(b) => slab(o, d, b.min().into(), b.max().into(), limit),
                None => Some(0.0),
            };
            if let Some(enters) = enters {
                batches.push(Candidate {
                    enters,
                    to_mesh,
                    geometry: batch.geometry.clone(),
                    seen: batch.seen.clone(),
                });
            }
        }
        batches.sort_by(|a, b| a.enters.total_cmp(&b.enters));
        Ray {
            origin,
            dir: *dir,
            terrain,
            reach,
            batches,
            chain: self.install.0.clone(),
        }
    }
}

/// A ray cast into what was drawn, still to be followed through the model batches it enters.
pub struct Ray {
    origin: Vec3,
    dir: Vec3,
    terrain: Option<(f32, Seen, (u32, u32))>,
    reach: f32,
    /// Nearest entry first.
    batches: Vec<Candidate>,
    chain: Arc<Chain>,
}

struct Candidate {
    enters: f32,
    to_mesh: Affine3A,
    geometry: Arc<RenderSubmesh>,
    seen: Seen,
}

impl Ray {
    /// The nearest face the ray meets. A face is met from the side it is drawn from, and a batch
    /// that turns to the camera or is two-sided from either; a batch that cuts out or blends only
    /// where its texture, read from the install, passes the client's alpha key. Bodies, liquids,
    /// particles, the horizon and the sky are not met.
    pub fn first(self) -> Option<Sighting> {
        let mut paints = CoverageReader::new(&self.chain);
        let mut best = self.terrain.map(|(t, seen, tile)| (t, seen, Some(tile)));
        for c in &self.batches {
            let limit = best.as_ref().map_or(self.reach, |b| b.0);
            if c.enters >= limit {
                break;
            }
            if let Some(t) = batch_hit(c, self.origin, self.dir, limit, &mut paints) {
                best = Some((t, c.seen.clone(), None));
            }
        }
        best.map(|(distance, seen, tile)| {
            let point = self.origin + self.dir * distance;
            let [x, y, _] = bevy_to_wow(point);
            Sighting {
                point,
                distance,
                seen,
                tile: tile.unwrap_or_else(|| wdt::world_to_tile(x, y)),
            }
        })
    }
}

/// A chunk's nearest front face along the ray, in WoW's axes, nearer than `limit`.
fn chunk_hit(chunk: &ChunkMesh, o: Vec3, d: Vec3, limit: f32) -> Option<f32> {
    let nw = Vec3::from(*chunk.positions.first()?);
    let (lo, hi) = (nw - Vec3::new(CHUNK_SIZE, CHUNK_SIZE, 0.0), nw);
    let (floor, roof) = (f32::NEG_INFINITY, f32::INFINITY);
    slab(o, d, lo.with_z(floor), hi.with_z(roof), limit)?;
    let (z_lo, z_hi) = chunk
        .positions
        .iter()
        .fold((roof, floor), |(a, b), p| (a.min(p[2]), b.max(p[2])));
    slab(o, d, lo.with_z(z_lo), hi.with_z(z_hi), limit)?;
    let mut best = None;
    for tri in chunk.indices.as_chunks::<3>().0 {
        let corner = |i: u32| chunk.positions.get(i as usize).map(|p| Vec3::from(*p) - o);
        let [Some(a), Some(b), Some(c)] = tri.map(corner) else {
            continue;
        };
        if let Some(hit) = triangle_hit(d, [a, b, c], false)
            && hit.t < best.unwrap_or(limit)
        {
            best = Some(hit.t);
        }
    }
    best
}

/// A batch's nearest face along the ray that paints where it is met, nearer than `limit`. The ray
/// is carried into the batch's own space, where its parameter is still distance in the world.
fn batch_hit(
    c: &Candidate,
    origin: Vec3,
    dir: Vec3,
    limit: f32,
    paints: &mut CoverageReader<'_>,
) -> Option<f32> {
    let (o, d) = (
        c.to_mesh.transform_point3(origin),
        c.to_mesh.transform_vector3(dir),
    );
    let g = &*c.geometry;
    let pivot = g
        .billboard
        .as_ref()
        .map_or(Vec3::ZERO, |b| wow_to_bevy(b.pivot));
    let two_sided = g.two_sided || g.billboard.is_some();
    let mut coverage = None;
    let mut best = None;
    for tri in g.indices.as_chunks::<3>().0 {
        let corner = |i: u32| {
            g.positions
                .get(i as usize)
                .map(|p| wow_to_bevy(*p) - pivot - o)
        };
        let [Some(a), Some(b), Some(e)] = tri.map(corner) else {
            continue;
        };
        let Some(hit) = triangle_hit(d, [a, b, e], two_sided) else {
            continue;
        };
        if hit.t >= best.unwrap_or(limit) {
            continue;
        }
        let painted = match coverage.get_or_insert_with(|| paints.coverage(g)) {
            Ok(None) => false,
            Ok(Some(Coverage::Alpha(alpha))) => {
                let uv = |i: u32| {
                    g.uvs
                        .get(i as usize)
                        .map_or(Vec2::ZERO, |uv| Vec2::from(*uv))
                };
                let [ua, ub, uc] = tri.map(uv);
                let at = ua * (1.0 - hit.u - hit.v) + ub * hit.u + uc * hit.v;
                alpha.sample(at.x, at.y, g.wrap_x, g.wrap_y) >= ALPHA_KEY_REF
            }
            Ok(Some(Coverage::Full)) | Err(_) => true,
        };
        if painted {
            best = Some(hit.t);
        }
    }
    best
}

/// Where a ray meets a triangle: its distance, and the weights of the second and third corners.
struct Hit {
    t: f32,
    u: f32,
    v: f32,
}

/// Where along `d` from the origin the triangle is met: from its front, the side it winds
/// counter-clockwise to, unless it is `two_sided`.
fn triangle_hit(d: Vec3, [a, b, c]: [Vec3; 3], two_sided: bool) -> Option<Hit> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 || (!two_sided && det < 0.0) {
        return None;
    }
    let s = -a;
    let u = s.dot(p) / det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) / det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) / det;
    (t > 0.0).then_some(Hit { t, u, v })
}

/// Where the ray enters the box, if it does nearer than `limit`.
fn slab(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3, limit: f32) -> Option<f32> {
    let (mut near, mut far) = (0.0_f32, limit);
    for axis in 0..3 {
        let (o, d, lo, hi) = (o[axis], d[axis], lo[axis], hi[axis]);
        if d == 0.0 {
            if o < lo || o > hi {
                return None;
            }
            continue;
        }
        let (t0, t1) = ((lo - o) / d, (hi - o) / d);
        near = near.max(t0.min(t1));
        far = far.min(t0.max(t1));
        if near > far {
            return None;
        }
    }
    Some(near)
}

#[cfg(test)]
mod tests;
