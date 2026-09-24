//! The render-ready view of World of Warcraft 1.12.1 M2 and WMO models: batches, bounds,
//! collision, animation. Positions are in the model's own space: WoW axes, Z up.

use std::collections::HashMap;

use mpq::Chain;

mod anim_summary;
mod animation;
mod art_extent;
mod batch_parts;
mod bone;
mod bounds;
mod camera;
mod collision;
mod draw_order;
#[cfg(test)]
mod effects_install;
mod emit_timing;
mod error;
mod global_seq;
mod ground_quad;
mod key_anim;
mod lights;
mod liquid;
mod m2_batches;
mod mat_anim;
mod particle_curves;
mod particles;
mod raster;
mod ribbons;
mod skeleton;
mod submesh;
mod tex_anim;
mod value_track;
mod wmo_group;
mod wmo_liquid;
mod wmo_mocv;
mod wmo_root;

pub use anim_summary::*;
pub use animation::*;
pub use art_extent::*;
pub use batch_parts::*;
pub use bone::*;
pub use bounds::*;
pub use camera::*;
pub use collision::*;
pub use draw_order::*;
pub use emit_timing::*;
pub use error::Error;
pub use global_seq::*;
pub use ground_quad::*;
pub use key_anim::*;
pub use lights::*;
pub use liquid::{LiquidKind, LiquidMesh};
pub use m2_batches::*;
pub use mat_anim::*;
pub use particle_curves::*;
pub use particles::{ParticleBlend, ParticleEmitterDef, ParticleShape, parse_m2_particle_emitters};
pub use ribbons::*;
pub use skeleton::*;
pub use submesh::*;
pub use tex_anim::*;
pub use value_track::{TrackValue, ValueTrack};
pub use wmo_group::*;
pub use wmo_liquid::*;
pub use wmo_mocv::*;
pub use wmo_root::*;

fn remap_submesh(
    global_indices: impl Iterator<Item = u32>,
    vertex: impl Fn(u32) -> ([f32; 3], [f32; 3], [f32; 2], [f32; 4]),
    texture: Option<String>,
    blend: ModelBlend,
    two_sided: bool,
    interior: bool,
    emissive: bool,
) -> (RenderSubmesh, Vec<u32>) {
    let mut map: HashMap<u32, u32> = HashMap::new();
    let (mut positions, mut normals, mut uvs, mut indices, mut vertex_colors) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut globals: Vec<u32> = Vec::new();
    for g in global_indices {
        let local = *map.entry(g).or_insert_with(|| {
            let (p, n, uv, c) = vertex(g);
            positions.push(p);
            normals.push(n);
            uvs.push(uv);
            vertex_colors.push(c);
            globals.push(g);
            (positions.len() - 1) as u32
        });
        indices.push(local);
    }
    (
        RenderSubmesh {
            positions,
            normals,
            uvs,
            indices,
            texture,
            blend,
            two_sided,
            vertex_colors,
            interior,
            emissive,
            ..RenderSubmesh::default()
        },
        globals,
    )
}

/// The client loads `.mdx` and `.mdl` model paths as `.m2`.
pub(crate) fn model_path(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    match lower
        .strip_suffix(".mdx")
        .or_else(|| lower.strip_suffix(".mdl"))
    {
        Some(stem) => format!("{stem}.m2"),
        None => lower,
    }
}

pub(crate) fn le_u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub(crate) fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

pub(crate) fn le_f32(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Loads a model by path: a `.wmo` through [`load_wmo`], anything else through [`load_m2_mesh`].
pub fn load_object_model(chain: &Chain, raw_path: &str) -> Result<Vec<RenderSubmesh>, Error> {
    if raw_path.to_ascii_lowercase().ends_with(".wmo") {
        load_wmo(chain, raw_path)
    } else {
        load_m2_mesh(chain, raw_path)
    }
}
