use std::collections::HashMap;
use std::io;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext, RenderAssetUsages};
use bevy::camera::primitives::{Aabb, MeshAabb};
use bevy::image::{CompressedImageFormats, Image};
use bevy::math::Vec3;
use bevy::mesh::{Indices, Mesh, MeshVertexAttribute, PrimitiveTopology};
use bevy::prelude::Handle;
use bevy::reflect::TypePath;
use terrain::{ALPHA_MAP_SIZE, ChunkMesh, SHADOW_MAP_SIZE};

use crate::coords::wow_to_bevy;
use crate::layers::{self, FALLBACK_LAYER, Levels, RawLayer};
use crate::source::MPQ_SOURCE;

const LAYER_INDICES: MeshVertexAttribute = Mesh::ATTRIBUTE_COLOR;
const MAP_INDICES: MeshVertexAttribute = Mesh::ATTRIBUTE_UV_1;
/// What the terrain shader reads as "no shadow map".
const NO_SHADOW: f32 = -1.0;

/// An ADT tile ready to draw: its terrain as one mesh, and the texture arrays its material reads.
#[derive(Asset, TypePath)]
pub struct AdtTile {
    /// `None` when holes cover every chunk.
    pub mesh: Option<(Handle<Mesh>, Aabb)>,
    pub layer_array: Handle<Image>,
    pub alpha_array: Handle<Image>,
    pub shadow_array: Handle<Image>,
}

#[derive(Clone, Copy)]
struct ChunkArrayLayers {
    /// Unused slots repeat the first; their alpha weight is 0.
    ground: [u32; 4],
    alpha: Option<u32>,
    shadow: Option<u32>,
}

#[derive(TypePath)]
pub(crate) struct AdtLoader {
    formats: CompressedImageFormats,
}

impl AdtLoader {
    pub(crate) fn new(formats: CompressedImageFormats) -> Self {
        Self { formats }
    }
}

impl AssetLoader for AdtLoader {
    type Asset = AdtTile;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<AdtTile, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let tile = terrain::adt_to_tile_mesh(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

        let mut layers: Vec<RawLayer> = Vec::new();
        let mut layer_index: HashMap<String, u32> = HashMap::new();
        let (mut alpha, mut alpha_count) = (Vec::new(), 0u32);
        let (mut shadow, mut shadow_count) = (Vec::new(), 0u32);
        let mut drawn = Vec::with_capacity(tile.chunks.len());
        for chunk in &tile.chunks {
            if chunk.indices.len() < 3 {
                continue;
            }
            let mut ground = [FALLBACK_LAYER; 4];
            for (slot, name) in chunk.layer_textures.iter().take(4).enumerate() {
                let key = name.replace('/', "\\").to_ascii_lowercase();
                ground[slot] = if let Some(&i) = layer_index.get(&key) {
                    i
                } else if let Some(layer) = read_layer(ctx, &key).await {
                    layers.push(layer);
                    let i = FALLBACK_LAYER + layers.len() as u32;
                    layer_index.insert(key, i);
                    i
                } else {
                    FALLBACK_LAYER
                };
            }
            for slot in chunk.layer_textures.len().min(4)..4 {
                ground[slot] = ground[0];
            }
            let alpha_layer = chunk.alpha_map.as_ref().map(|rgba| {
                alpha.extend_from_slice(rgba);
                alpha_count += 1;
                alpha_count - 1
            });
            let shadow_layer = chunk.shadow.as_ref().map(|map| {
                shadow.extend_from_slice(map);
                shadow_count += 1;
                shadow_count - 1
            });
            let array_layers = ChunkArrayLayers {
                ground,
                alpha: alpha_layer,
                shadow: shadow_layer,
            };
            drawn.push((chunk, array_layers));
        }
        if alpha_count == 0 {
            alpha = vec![0; (ALPHA_MAP_SIZE * ALPHA_MAP_SIZE * 4) as usize];
            alpha_count = 1;
        }
        if shadow_count == 0 {
            shadow = vec![0; (SHADOW_MAP_SIZE * SHADOW_MAP_SIZE) as usize];
            shadow_count = 1;
        }
        let layer_count = layers.len() as u32 + 1;
        let packed = layers::pack_layers(layers, self.formats);
        let mesh = tile_mesh(&drawn).map(|mesh| {
            let aabb = mesh.compute_aabb().unwrap_or_default();
            (ctx.add_labeled_asset("mesh".into(), mesh), aabb)
        });
        Ok(AdtTile {
            mesh,
            layer_array: ctx.add_labeled_asset(
                "layer_array".into(),
                layers::layer_array(layer_count, packed),
            ),
            alpha_array: ctx.add_labeled_asset(
                "alpha_array".into(),
                layers::alpha_array(alpha_count, alpha),
            ),
            shadow_array: ctx.add_labeled_asset(
                "shadow_array".into(),
                layers::shadow_array(shadow_count, shadow),
            ),
        })
    }

    fn extensions(&self) -> &[&str] {
        &["adt"]
    }
}

/// A ground texture's `_s` variant, whose alpha is the sheen mask, or else the base, read matte.
async fn read_layer(ctx: &mut LoadContext<'_>, key: &str) -> Option<RawLayer> {
    if let Some(stem) = key.strip_suffix(".blp")
        && let Ok(bytes) = ctx
            .read_asset_bytes(mpq_url(&format!("{stem}_s.blp")))
            .await
        && let Ok(blp) = blp::decode_native(&bytes)
    {
        return Some(RawLayer {
            levels: Levels::from(blp),
            matte: false,
        });
    }
    let bytes = ctx.read_asset_bytes(mpq_url(key)).await.ok()?;
    let blp = blp::decode_native(&bytes).ok()?;
    Some(RawLayer {
        levels: Levels::from(blp),
        matte: true,
    })
}

fn mpq_url(key: &str) -> String {
    format!("{MPQ_SOURCE}://{}", key.replace('\\', "/"))
}

fn tile_mesh(chunks: &[(&ChunkMesh, ChunkArrayLayers)]) -> Option<Mesh> {
    if chunks.is_empty() {
        return None;
    }
    let total: usize = chunks.iter().map(|(c, _)| c.positions.len()).sum();
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(total);
    let mut normals: Vec<[f32; 3]> = Vec::with_capacity(total);
    let mut uvs: Vec<[f32; 2]> = Vec::with_capacity(total);
    let mut map_indices: Vec<[f32; 2]> = Vec::with_capacity(total);
    let mut layer_indices: Vec<[f32; 4]> = Vec::with_capacity(total);
    let mut indices: Vec<u32> = Vec::new();
    for (chunk, array_layers) in chunks {
        let base = positions.len() as u32;
        let chunk_positions: Vec<[f32; 3]> = chunk
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        if chunk.normals.len() == chunk.positions.len() {
            normals.extend(chunk.normals.iter().map(|n| wow_to_bevy(*n).to_array()));
        } else {
            normals.extend(area_weighted_normals(&chunk_positions, &chunk.indices));
        }
        let n = chunk_positions.len();
        // A chunk without an alpha map draws one texture in all four slots, so any map will do.
        let alpha = array_layers.alpha.unwrap_or(0) as f32;
        let shadow = array_layers.shadow.map_or(NO_SHADOW, |s| s as f32);
        map_indices.extend(std::iter::repeat_n([alpha, shadow], n));
        layer_indices.extend(std::iter::repeat_n(
            array_layers.ground.map(|l| l as f32),
            n,
        ));
        uvs.extend_from_slice(&chunk.uvs);
        indices.extend(chunk.indices.iter().map(|i| i + base));
        positions.extend(chunk_positions);
    }
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(MAP_INDICES, map_indices);
    mesh.insert_attribute(LAYER_INDICES, layer_indices);
    mesh.insert_indices(Indices::U32(indices));
    Some(mesh)
}

fn area_weighted_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![Vec3::ZERO; positions.len()];
    for tri in indices.as_chunks::<3>().0 {
        let [a, b, c] = tri.map(|i| i as usize);
        let (va, vb, vc) = (
            Vec3::from(positions[a]),
            Vec3::from(positions[b]),
            Vec3::from(positions[c]),
        );
        let n = (vb - va).cross(vc - va);
        acc[a] += n;
        acc[b] += n;
        acc[c] += n;
    }
    acc.into_iter()
        .map(|n| n.normalize_or_zero().to_array())
        .collect()
}
