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

pub mod frame;

use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::source::{Install, MPQ_SOURCE};
use crate::stream::Streamer;

/// What a ray met. A file is its path in the install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// A chunk of the terrain of the tile's ADT.
    Terrain { column: u32, row: u32 },
    /// An M2 the map places, by its placement's unique id in the ADTs.
    Doodad { file: Arc<str>, unique_id: u32 },
    /// A group of a WMO the map places.
    Building {
        file: Arc<str>,
        unique_id: u32,
        group: u16,
    },
    /// An M2 a WMO places, by its index among the WMO's doodads.
    Prop {
        file: Arc<str>,
        building_file: Arc<str>,
        building_unique_id: u32,
        doodad: usize,
    },
}

impl Seen {
    /// The unique id of the placement the map's files place it under: a building's for its own
    /// doodads.
    pub fn placement(&self) -> Option<u32> {
        match self {
            Seen::Terrain { .. } => None,
            Seen::Doodad { unique_id, .. } | Seen::Building { unique_id, .. } => Some(*unique_id),
            Seen::Prop {
                building_unique_id, ..
            } => Some(*building_unique_id),
        }
    }
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

/// What a ray meets short of a distance.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nearer {
    /// Each model batch met in front of the terrain, nearest first.
    pub models: Vec<Seen>,
    pub terrain: bool,
}

#[derive(Component, Clone)]
pub(crate) struct Meetable {
    pub(crate) geometry: Arc<RenderSubmesh>,
    pub(crate) seen: Seen,
}

pub(crate) fn install_path(url: &str) -> Arc<str> {
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
    /// is found now, and the model batches the ray enters nearer than it are kept for
    /// [`Cast::first`], which can run on any thread. Cast after `PostUpdate`'s visibility check,
    /// they are the batches drawn this frame.
    pub fn cast(&self, origin: Vec3, dir: Dir3, reach: f32) -> Cast {
        let mut terrain: Option<(f32, Seen, (u32, u32))> = None;
        let (from_wow, dir_wow) = (
            Vec3::from(bevy_to_wow(origin)),
            Vec3::from(bevy_to_wow(*dir)),
        );
        for (tile, handle) in self.streamer.arrived() {
            for chunk in self.tiles.get(handle).into_iter().flat_map(|t| &t.chunks) {
                let limit = terrain.as_ref().map_or(reach, |t| t.0);
                if let Some(t) = chunk_hit(chunk, from_wow, dir_wow, limit) {
                    let seen = Seen::Terrain {
                        column: chunk.index_x,
                        row: chunk.index_y,
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
                Some(b) => box_entry(o, d, b.min().into(), b.max().into(), limit),
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
        Cast {
            origin,
            dir: *dir,
            terrain,
            reach,
            by_entry: batches,
            chain: self.install.0.clone(),
        }
    }

    /// The model batches drawn this frame whose boxes come within `radius` of `at`, in Bevy's
    /// axes.
    pub fn within(&self, at: Vec3, radius: f32) -> Vec<Seen> {
        let mut out = Vec::new();
        for (batch, placed, drawn, bound) in &self.batches {
            if !drawn.get() {
                continue;
            }
            let affine = placed.affine();
            let local = affine.inverse().transform_point3(at);
            let gap = bound.map_or(local.length(), |b| {
                box_gap(local, b.min().into(), b.max().into())
            });
            if gap * affine.matrix3.x_axis.length() <= radius {
                out.push(batch.seen.clone());
            }
        }
        out
    }
}

/// A ray cast into what was drawn, still to be followed through the model batches it enters.
pub struct Cast {
    origin: Vec3,
    dir: Vec3,
    terrain: Option<(f32, Seen, (u32, u32))>,
    reach: f32,
    by_entry: Vec<Candidate>,
    chain: Arc<Chain>,
}

struct Candidate {
    enters: f32,
    to_mesh: Affine3A,
    geometry: Arc<RenderSubmesh>,
    seen: Seen,
}

impl Cast {
    /// The nearest face the ray meets. A face is met from the side it is drawn from, and a batch
    /// that turns to the camera or is two-sided from either. A batch that cuts out or blends is met
    /// only where its texture, read from the install, passes the client's alpha key: nowhere when
    /// it has no texture, everywhere when its texture does not read. A batch that multiplies what
    /// is behind it is never met, and an animated one is met as it stands at rest. Bodies,
    /// liquids, particles, the horizon and the sky are not met.
    pub fn first(self) -> Option<Sighting> {
        let mut paints = CoverageReader::new(&self.chain);
        let mut best = self.terrain.map(|(t, seen, tile)| (t, seen, Some(tile)));
        for c in &self.by_entry {
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

    /// Every model batch the ray meets short of `distance`, as [`Cast::first`] meets them, and
    /// whether the terrain comes first.
    pub fn nearer(self, distance: f32) -> Nearer {
        let mut paints = CoverageReader::new(&self.chain);
        let terrain = self.terrain.as_ref().is_some_and(|t| t.0 < distance);
        let limit = self
            .terrain
            .as_ref()
            .map_or(self.reach, |t| t.0)
            .min(distance);
        let mut met: Vec<(f32, Seen)> = self
            .by_entry
            .iter()
            .take_while(|c| c.enters < limit)
            .filter_map(|c| {
                batch_hit(c, self.origin, self.dir, limit, &mut paints).map(|t| (t, c.seen.clone()))
            })
            .collect();
        met.sort_by(|a, b| a.0.total_cmp(&b.0));
        Nearer {
            models: met.into_iter().map(|(_, seen)| seen).collect(),
            terrain,
        }
    }
}

fn chunk_hit(chunk: &ChunkMesh, from_wow: Vec3, dir_wow: Vec3, limit: f32) -> Option<f32> {
    let nw = Vec3::from(*chunk.positions.first()?);
    let (lo, hi) = (nw - Vec3::new(CHUNK_SIZE, CHUNK_SIZE, 0.0), nw);
    let (floor, roof) = (f32::NEG_INFINITY, f32::INFINITY);
    box_entry(from_wow, dir_wow, lo.with_z(floor), hi.with_z(roof), limit)?;
    let (z_lo, z_hi) = chunk
        .positions
        .iter()
        .fold((roof, floor), |(a, b), p| (a.min(p[2]), b.max(p[2])));
    box_entry(from_wow, dir_wow, lo.with_z(z_lo), hi.with_z(z_hi), limit)?;
    let mut best = None;
    for tri in chunk.indices.as_chunks::<3>().0 {
        let corner = |i: u32| {
            let p = chunk.positions.get(i as usize)?;
            Some(Vec3::from(*p) - from_wow)
        };
        let [Some(a), Some(b), Some(c)] = tri.map(corner) else {
            continue;
        };
        if let Some(hit) = triangle_hit(dir_wow, [a, b, c], false)
            && hit.distance < best.unwrap_or(limit)
        {
            best = Some(hit.distance);
        }
    }
    best
}

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
    let at_rest = Vec2::from(g.uv_anim.as_ref().map_or([0.0; 2], |a| a.sample(0.0)));
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
        if hit.distance >= best.unwrap_or(limit) {
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
                let on_mesh =
                    ua * (1.0 - hit.second - hit.third) + ub * hit.second + uc * hit.third;
                let at = on_mesh + at_rest;
                alpha.sample(at.x, at.y, g.wrap_x, g.wrap_y) >= ALPHA_KEY_REF
            }
            Ok(Some(Coverage::Full)) | Err(_) => true,
        };
        if painted {
            best = Some(hit.distance);
        }
    }
    best
}

struct Hit {
    distance: f32,
    second: f32,
    third: f32,
}

fn triangle_hit(d: Vec3, from_origin: [Vec3; 3], two_sided: bool) -> Option<Hit> {
    let [a, b, c] = from_origin;
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let facing_the_ray = e1.dot(p);
    if facing_the_ray == 0.0 || (!two_sided && facing_the_ray < 0.0) {
        return None;
    }
    let s = -a;
    let second = s.dot(p) / facing_the_ray;
    if !(0.0..=1.0).contains(&second) {
        return None;
    }
    let q = s.cross(e1);
    let third = d.dot(q) / facing_the_ray;
    if third < 0.0 || second + third > 1.0 {
        return None;
    }
    let distance = e2.dot(q) / facing_the_ray;
    (distance > 0.0).then_some(Hit {
        distance,
        second,
        third,
    })
}

/// How far `p` lies outside the box, 0 within it.
fn box_gap(p: Vec3, lo: Vec3, hi: Vec3) -> f32 {
    (lo - p).max(p - hi).max(Vec3::ZERO).length()
}

fn box_entry(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3, limit: f32) -> Option<f32> {
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
