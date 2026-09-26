//! Ground clutter: the grass, flowers and stones the client scatters over each chunk of terrain by
//! its textures' ground effects, meshed only near the camera and faded out by view depth.

mod scatter;

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::math::ops;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use model::RenderSubmesh;
use mpq::Chain;
use terrain::{CHUNK_SIZE, ChunkMesh, TILE_SIZE};

use crate::adt::AdtTile;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::light::LightBuffer;
use crate::model_material::{ModelMaterial, clutter_material};
use crate::source::Repeat;
use crate::stream::Streamer;
use crate::texture::blp_image;
use crate::view::WorldCamera;
use crate::{Install, Residency};
use scatter::{Effects, Tuft};

/// Cells a chunk draws, with repeats: the client's options at Medium, the nearest of their stops
/// to the density a fresh install detects that is no sparser.
const CELLS_PER_CHUNK: u32 = 32;
/// The view depth the client's ground clutter has faded out by.
const FADE_FAR: f32 = 70.0;
/// Yards past the fade's reach where a chunk is built, so none of it shows before it is.
const BUILD_MARGIN: f32 = 8.0;
/// Yards between building and tearing down, so a chunk on the edge does not flip.
const HYSTERESIS: f32 = 6.0;
/// How far a tuft reaches above the ground.
const TUFT_HEIGHT: f32 = 3.0;
/// Most chunks built in a frame, so arriving in a dense place spreads over a few.
const BUILDS_PER_FRAME: usize = 8;
/// A tuft in the ground's baked shadow is this grey.
const SHADOWED: f32 = 192.0 / 255.0;
const VERTICES: usize = 145;

/// The ground effects; absent, and no clutter, when the install's tables do not read.
#[derive(Resource)]
pub(crate) struct GroundEffects(Effects);

pub(crate) fn load_effects(mut commands: Commands<'_, '_>, install: Res<'_, Install>) {
    match Effects::read(&install.0) {
        Ok(effects) => commands.insert_resource(GroundEffects(effects)),
        Err(e) => warn!("no ground effect tables, so no ground clutter: {e}"),
    }
}

/// The chunks whose clutter is built, by their place on the map, and what it is built from: each
/// model and texture read once.
#[derive(Resource, Default)]
pub(crate) struct Clutter {
    built: BTreeMap<(u32, u32), Built>,
    models: HashMap<Arc<str>, Arc<[RenderSubmesh]>>,
    textures: HashMap<(String, bool, bool), Option<Handle<Image>>>,
    materials: HashMap<Option<AssetId<Image>>, Handle<ModelMaterial>>,
}

struct Built {
    bounds: (Vec3, Vec3),
    parts: Vec<Entity>,
}

struct Wanted<'a> {
    distance_squared: f32,
    key: (u32, u32),
    tile: (u32, u32),
    chunk: &'a ChunkMesh,
    bounds: (Vec3, Vec3),
}

/// Builds each chunk's clutter once its box comes within reach of the fade, nearest first, and
/// tears it down past that, where the fade has left nothing. The reach is along the frustum's
/// corner, since the fade is by view depth.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_clutter(
    mut commands: Commands<'_, '_>,
    camera: Query<'_, '_, (&Transform, &Projection), With<WorldCamera>>,
    effects: Option<Res<'_, GroundEffects>>,
    ground: (Res<'_, Streamer>, Res<'_, Assets<AdtTile>>),
    install: Res<'_, Install>,
    light: Option<Res<'_, LightBuffer>>,
    support: Option<Res<'_, CompressedImageFormatSupport>>,
    mut clutter: ResMut<'_, Clutter>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut materials: ResMut<'_, Assets<ModelMaterial>>,
    mut residency: ResMut<'_, Residency>,
) {
    let Some(effects) = effects else {
        residency.clutter = true;
        return;
    };
    let (Ok((camera, projection)), Some(light)) = (camera.single(), light) else {
        return;
    };
    let fallback = PerspectiveProjection::default();
    let view = match projection {
        Projection::Perspective(p) => p,
        _ => &fallback,
    };
    let reach = FADE_FAR * corner_reach(view.fov, view.aspect_ratio);
    let build = square(reach + BUILD_MARGIN);
    let drop = square(reach + BUILD_MARGIN + HYSTERESIS);
    let eye = camera.translation;
    clutter.built.retain(|_, b| {
        let keep = box_distance_squared(eye, b.bounds) <= drop;
        if !keep {
            for &e in &b.parts {
                commands.entity(e).try_despawn();
            }
        }
        keep
    });
    let (streamer, adts) = ground;
    let mut wanted = Vec::new();
    for (tile, handle) in streamer.arrived() {
        for chunk in adts.get(handle).into_iter().flat_map(|a| &a.chunks) {
            let key = (tile.0 * 16 + chunk.index_x, tile.1 * 16 + chunk.index_y);
            if chunk.positions.len() < VERTICES
                || footprint_distance_squared(eye, chunk) > build
                || clutter.built.contains_key(&key)
            {
                continue;
            }
            let bounds = bounds(chunk);
            let distance_squared = box_distance_squared(eye, bounds);
            if distance_squared <= build {
                wanted.push(Wanted {
                    distance_squared,
                    key,
                    tile,
                    chunk,
                    bounds,
                });
            }
        }
    }
    wanted.sort_by(|a, b| {
        a.distance_squared
            .total_cmp(&b.distance_squared)
            .then(a.key.cmp(&b.key))
    });
    let formats = support.map_or(CompressedImageFormats::NONE, |s| s.0);
    let mut builder = Builder {
        chain: &install.0,
        formats,
        light: &light.0,
        meshes: &mut meshes,
        images: &mut images,
        materials: &mut materials,
        commands: &mut commands,
    };
    let (mut spent, mut left) = (0, 0);
    for w in wanted {
        if spent == BUILDS_PER_FRAME {
            left += 1;
            continue;
        }
        let tufts = scatter::scatter(w.chunk, w.tile, &effects.0, CELLS_PER_CHUNK);
        let parts = builder.chunk(&mut clutter, w.chunk, tufts);
        spent += usize::from(!parts.is_empty());
        clutter.built.insert(
            w.key,
            Built {
                bounds: w.bounds,
                parts,
            },
        );
    }
    residency.clutter = left == 0;
}

struct Builder<'a, 'w, 's> {
    chain: &'a Chain,
    formats: CompressedImageFormats,
    light: &'a Buffer,
    meshes: &'a mut Assets<Mesh>,
    images: &'a mut Assets<Image>,
    materials: &'a mut Assets<ModelMaterial>,
    commands: &'a mut Commands<'w, 's>,
}

struct Shaded {
    tuft: Tuft,
    tint: f32,
    ground: [f32; 3],
}

impl Builder<'_, '_, '_> {
    /// One mesh for each batch of each model, with every one of its tufts in it: the tuft's place,
    /// the ground's shadow and the ground's normal baked into its vertices.
    fn chunk(&mut self, clutter: &mut Clutter, chunk: &ChunkMesh, tufts: Vec<Tuft>) -> Vec<Entity> {
        let mut by_model: BTreeMap<Arc<str>, Vec<Shaded>> = BTreeMap::new();
        for tuft in tufts {
            let shadowed = chunk.mcsh_shadowed_at(tuft.position) == Some(true);
            let ground = wow_to_bevy(ground_normal(chunk, tuft.position)).to_array();
            by_model
                .entry(tuft.model.clone())
                .or_default()
                .push(Shaded {
                    tint: if shadowed { SHADOWED } else { 1.0 },
                    ground,
                    tuft,
                });
        }
        let mut parts = Vec::new();
        for (path, tufts) in &by_model {
            let batches = clutter
                .models
                .entry(path.clone())
                .or_insert_with(|| {
                    model::load_m2_mesh(self.chain, path)
                        .unwrap_or_default()
                        .into()
                })
                .clone();
            for batch in batches.iter() {
                let Some((mesh, aabb)) = merged(batch, tufts) else {
                    continue;
                };
                let texture = batch.texture.as_deref().and_then(|t| {
                    let repeat = Repeat {
                        u: batch.wrap_x,
                        v: batch.wrap_y,
                    };
                    self.texture(clutter, t, repeat)
                });
                let material = clutter
                    .materials
                    .entry(texture.as_ref().map(Handle::id))
                    .or_insert_with(|| {
                        let material = clutter_material(texture, FADE_FAR, self.light);
                        self.materials.add(material)
                    })
                    .clone();
                let mesh = self.meshes.add(mesh);
                parts.push(
                    self.commands
                        .spawn((Mesh3d(mesh), MeshMaterial3d(material), aabb))
                        .id(),
                );
            }
        }
        parts
    }

    fn texture(
        &mut self,
        clutter: &mut Clutter,
        path: &str,
        repeat: Repeat,
    ) -> Option<Handle<Image>> {
        clutter
            .textures
            .entry((path.to_ascii_lowercase(), repeat.u, repeat.v))
            .or_insert_with(|| {
                let bytes = self.chain.read(path).ok()?;
                let blp = blp::decode_native(&bytes).ok()?;
                Some(self.images.add(blp_image(blp, self.formats, repeat)))
            })
            .clone()
    }
}

fn merged(batch: &RenderSubmesh, tufts: &[Shaded]) -> Option<(Mesh, Aabb)> {
    if batch.positions.is_empty() || batch.uvs.len() != batch.positions.len() {
        return None;
    }
    let n = batch.positions.len() * tufts.len();
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(n);
    let mut normals = Vec::with_capacity(n);
    let mut colors = Vec::with_capacity(n);
    let mut uvs = Vec::with_capacity(n);
    let mut indices = Vec::with_capacity(batch.indices.len() * tufts.len());
    for s in tufts {
        let base = positions.len() as u32;
        let origin = wow_to_bevy(s.tuft.position);
        let turn = Quat::from_rotation_y(s.tuft.yaw);
        for (p, uv) in batch.positions.iter().zip(&batch.uvs) {
            positions.push((turn * (wow_to_bevy(*p) * s.tuft.scale) + origin).to_array());
            uvs.push(*uv);
            normals.push(s.ground);
            colors.push([s.tint, s.tint, s.tint, 1.0]);
        }
        indices.extend(batch.indices.iter().map(|i| base + i));
    }
    let aabb = Aabb::enclosing(positions.iter().copied().map(Vec3::from))?;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    Some((mesh, aabb))
}

/// The ground's normal under a point: the chunk's at the nearest vertex of its outer grid, or up
/// when it has none.
fn ground_normal(chunk: &ChunkMesh, at: [f32; 3]) -> [f32; 3] {
    let Some(nw) = chunk
        .positions
        .first()
        .filter(|_| chunk.normals.len() >= VERTICES)
    else {
        return [0.0, 0.0, 1.0];
    };
    let south = ((nw[0] - at[0]) / TILE_SIZE * 16.0).clamp(0.0, 1.0);
    let east = ((nw[1] - at[1]) / TILE_SIZE * 16.0).clamp(0.0, 1.0);
    let row = (south * 8.0).round() as usize;
    let col = (east * 8.0).round() as usize;
    chunk.normals[row * 17 + col]
}

/// The chunk's ground, raised by a tuft's height.
fn bounds(chunk: &ChunkMesh) -> (Vec3, Vec3) {
    let (mut lo, mut hi) = (Vec3::MAX, Vec3::MIN);
    for &p in &chunk.positions {
        let v = wow_to_bevy(p);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    hi.y += TUFT_HEIGHT;
    (lo, hi)
}

/// The squared distance across the ground to the chunk's square, never more than to its box.
fn footprint_distance_squared(eye: Vec3, chunk: &ChunkMesh) -> f32 {
    let [x, y, _] = bevy_to_wow(eye);
    let [x0, y0, _] = chunk.positions[0];
    let off = |v: f32, far: f32| (far - CHUNK_SIZE - v).max(v - far).max(0.0);
    square(off(x, x0)) + square(off(y, y0))
}

fn box_distance_squared(p: Vec3, (lo, hi): (Vec3, Vec3)) -> f32 {
    (lo - p).max(p - hi).max(Vec3::ZERO).length_squared()
}

/// How much farther than the view depth the frustum's corner reaches.
fn corner_reach(fov_y: f32, aspect: f32) -> f32 {
    let tan_v = ops::tan(fov_y * 0.5);
    let tan_h = tan_v * aspect;
    (1.0 + tan_v * tan_v + tan_h * tan_h).sqrt()
}

fn square(v: f32) -> f32 {
    v * v
}

#[cfg(test)]
mod tests;
