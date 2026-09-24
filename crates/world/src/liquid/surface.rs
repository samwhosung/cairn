//! The liquid surfaces drawn: one material per kind, renderer and scroll over a texture array of
//! the kind's frames, and the flat meshes spawned with their terrain tile or their building.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::MeshAabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::{Indices, MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use terrain::{LiquidKind, LiquidMesh};

use super::query::{LiquidSource, WmoPool, wet_footprint};
use super::{FoamPatch, LiquidClock, frames};
use crate::Install;
use crate::coords::wow_to_bevy;
use crate::light::LightBuffer;
use crate::portal::WmoGroupVis;
use crate::wmo::WmoRooms;

pub type LiquidMaterial = ExtendedMaterial<StandardMaterial, LiquidExtension>;

/// The water pass's place in the transparent sort: after every far-side transparent, before
/// every other one.
const WATER_BIAS: f32 = -2.0e4;
/// The exponent of water's own sun sheen.
const WATER_SHININESS: f32 = 6.0;

/// Bevy packs the uniforms onto one binding in field order, as `liquid.wgsl` lays them out.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct LiquidExtension {
    #[texture(100, dimension = "2d_array", visibility(fragment))]
    #[sampler(101, visibility(fragment))]
    pub frames: Handle<Image>,
    #[uniform(102)]
    pub kind: Vec4,
    #[uniform(102)]
    pub path: Vec4,
    #[uniform(102)]
    pub anim: Vec4,
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light: Buffer,
}

impl MaterialExtension for LiquidExtension {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/liquid/liquid.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/liquid/liquid.wgsl".into()
    }

    /// The water rung is a sort key only: as a depth bias it would move the waterline.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.bias.constant = 0;
        }
        Ok(())
    }
}

pub(super) fn plugin(app: &mut App) {
    embedded_asset!(app, "liquid.wgsl");
    app.add_plugins(MaterialPlugin::<LiquidMaterial>::default());
}

/// Which of the client's liquid renderers draws a surface.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum LiquidPath {
    Adt,
    WmoExterior,
    WmoInterior,
}

impl LiquidPath {
    fn wmo(interior: bool) -> Self {
        if interior {
            Self::WmoInterior
        } else {
            Self::WmoExterior
        }
    }

    fn shader_id(self) -> f32 {
        match self {
            Self::Adt => 0.0,
            Self::WmoExterior => 1.0,
            Self::WmoInterior => 2.0,
        }
    }
}

/// Only a building's magma and slime of type 6 and 7 scroll; terrain magma never does.
pub(super) fn scrolls(nibble: u8) -> bool {
    matches!(nibble, 6 | 7)
}

/// Each kind's frames: `(kind, XTextures directory, file stem, frames on disk)`.
const FRAME_SETS: &[(LiquidKind, &str, &str, u32)] = &[
    (LiquidKind::Still, "river", "lake_a", 30),
    (LiquidKind::Rapids, "river", "fast_a", 16),
    (LiquidKind::Ocean, "ocean", "ocean_h", 30),
    (LiquidKind::Magma, "lava", "lava", 30),
    (LiquidKind::Slime, "slime", "slime", 30),
];

/// The shared liquid materials, by kind, renderer and scroll.
#[derive(Resource, Default)]
pub(crate) struct LiquidAssets {
    materials: HashMap<(LiquidKind, LiquidPath, bool), Handle<LiquidMaterial>>,
}

impl LiquidAssets {
    fn material(
        &self,
        kind: LiquidKind,
        path: LiquidPath,
        scroll: bool,
    ) -> Option<Handle<LiquidMaterial>> {
        self.materials.get(&(kind, path, scroll)).cloned()
    }
}

/// Magma and slime draw opaque and write depth; water blends over what is behind it.
pub(super) fn setup_liquid(
    mut commands: Commands<'_, '_>,
    install: Res<'_, Install>,
    light: Option<Res<'_, LightBuffer>>,
    clock: Option<Res<'_, LiquidClock>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut materials: ResMut<'_, Assets<LiquidMaterial>>,
) {
    let Some(light) = light else {
        return;
    };
    let running = clock.is_none_or(|c| *c == LiquidClock::Running);
    let mut assets = LiquidAssets::default();
    for &(kind, dir, stem, count) in FRAME_SETS {
        let decoded = frames::read_frames(&install.0, dir, stem, count);
        let Some(array) = frames::frame_array(&decoded, !kind.is_fullbright()) else {
            warn!("no frames for {stem}: {kind:?} will not draw");
            continue;
        };
        let frames = images.add(array);
        let (alpha_mode, depth_bias) = if kind.is_fullbright() {
            (AlphaMode::Opaque, 0.0)
        } else {
            (AlphaMode::Blend, WATER_BIAS)
        };
        let scroll_lanes: &[bool] = if kind.is_fullbright() {
            &[false, true]
        } else {
            &[false]
        };
        for path in [
            LiquidPath::Adt,
            LiquidPath::WmoExterior,
            LiquidPath::WmoInterior,
        ] {
            for &scroll in scroll_lanes {
                let flag = |on: bool| if on { 1.0 } else { 0.0 };
                let material = materials.add(ExtendedMaterial {
                    base: StandardMaterial {
                        unlit: true,
                        alpha_mode,
                        cull_mode: None,
                        double_sided: true,
                        depth_bias,
                        ..StandardMaterial::default()
                    },
                    extension: LiquidExtension {
                        frames: frames.clone(),
                        kind: Vec4::new(
                            flag(kind.is_fullbright()),
                            flag(kind == LiquidKind::Ocean),
                            flag(path == LiquidPath::WmoInterior),
                            WATER_SHININESS,
                        ),
                        path: Vec4::new(path.shader_id(), 0.0, 0.0, 0.0),
                        anim: Vec4::new(0.0, decoded.len() as f32, flag(scroll), flag(running)),
                        light: light.0.clone(),
                    },
                });
                assets.materials.insert((kind, path, scroll), material);
            }
        }
    }
    commands.insert_resource(assets);
}

/// The flat mesh of one surface: positions in Bevy's axes, an up normal, the tiling UVs, the depth
/// coordinate in UV1's x and, for an indoor pool, its body colour in the vertex colour.
fn liquid_mesh(lq: &LiquidMesh, body_color: Option<[f32; 3]>) -> Mesh {
    let positions: Vec<[f32; 3]> = lq
        .positions
        .iter()
        .map(|p| wow_to_bevy(*p).to_array())
        .collect();
    let n = positions.len();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n]);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, lq.uvs.clone());
    let depth: Vec<[f32; 2]> = lq.depths.iter().map(|&d| [d, 0.0]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, depth);
    if let Some([r, g, b]) = body_color {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[r, g, b, 1.0]; n]);
    }
    mesh.insert_indices(Indices::U32(lq.indices.clone()));
    mesh
}

fn spawn_surface(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    lq: &LiquidMesh,
    material: Handle<LiquidMaterial>,
    body_color: Option<[f32; 3]>,
    transform: Transform,
    source: LiquidSource,
) -> Entity {
    let mesh = liquid_mesh(lq, body_color);
    let aabb = mesh.compute_aabb().unwrap_or_default();
    let mut e = commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(material),
        aabb,
        NoAutoAabb,
        transform,
        wet_footprint(lq, &transform, source),
    ));
    if !lq.kind.is_fullbright() {
        e.insert(FoamPatch);
    }
    e.id()
}

/// A terrain tile's liquid surfaces, one per chunk layer; their positions are already the world's.
pub(crate) fn spawn_adt_liquids<'a>(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    assets: &LiquidAssets,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
) -> Vec<Entity> {
    liquids
        .filter_map(|lq| {
            let material = assets.material(lq.kind, LiquidPath::Adt, false)?;
            Some(spawn_surface(
                commands,
                meshes,
                lq,
                material,
                None,
                Transform::IDENTITY,
                LiquidSource::AdtChunk,
            ))
        })
        .collect()
}

/// A placed building's pools, one per group. `instance` owns them when the building has portals
/// or an area id; the rest answer no subject and draw with no room.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_wmo_liquids(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    assets: &LiquidAssets,
    rooms: &WmoRooms,
    material_diff_colors: &[[f32; 3]],
    transform: Transform,
    instance: Entity,
) -> Vec<Entity> {
    let mut out = Vec::new();
    for (gi, lq) in rooms.group_liquids.iter().enumerate() {
        let Some(lq) = lq else { continue };
        let nav = rooms.group_nav.get(gi);
        let path = LiquidPath::wmo(nav.is_some_and(|g| g.interior));
        let scroll = scrolls(lq.sound_nibble);
        let Some(material) = assets.material(lq.kind, path, scroll) else {
            continue;
        };
        let body_color = (path == LiquidPath::WmoInterior && !lq.kind.is_fullbright())
            .then(|| {
                lq.material_id
                    .and_then(|id| material_diff_colors.get(usize::from(id)))
                    .copied()
            })
            .flatten();
        let pool = WmoPool::of(rooms, gi, instance, &transform);
        let e = spawn_surface(
            commands,
            meshes,
            lq,
            material,
            body_color,
            transform,
            LiquidSource::WmoGroup(pool),
        );
        commands.entity(e).insert(MeshTag(0));
        if pool.owner.is_some() {
            commands.entity(e).insert(WmoGroupVis {
                instance,
                groups: Arc::from([gi as u16].as_slice()),
            });
        }
        out.push(e);
    }
    out
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn only_building_magma_and_slime_of_six_and_seven_scroll() {
        assert!(scrolls(6) && scrolls(7));
        for n in [0u8, 1, 2, 3, 4, 5, 8, 0xf] {
            assert!(!scrolls(n), "{n}");
        }
    }

    #[test]
    fn the_mesh_carries_depth_in_uv1_and_the_body_colour_only_indoors() {
        let lq = LiquidMesh {
            grid: [2, 2],
            wet: vec![true],
            shared: vec![false],
            positions: vec![
                [0.0, 0.0, 1.0],
                [4.0, 0.0, 1.0],
                [0.0, 4.0, 1.0],
                [4.0, 4.0, 1.0],
            ],
            uvs: vec![[0.0; 2]; 4],
            depths: vec![0.25, 0.5, 0.75, 1.0],
            indices: vec![0, 2, 3, 0, 3, 1],
            sound_nibble: 4,
            material_id: Some(0),
            kind: LiquidKind::Still,
        };
        let mesh = liquid_mesh(&lq, None);
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(uv1)) =
            mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("UV1 carries the depth");
        };
        assert_eq!(uv1[2], [0.75, 0.0]);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_none());
        let mesh = liquid_mesh(&lq, Some([0.1, 0.2, 0.3]));
        assert!(mesh.attribute(Mesh::ATTRIBUTE_COLOR).is_some());
    }
}
