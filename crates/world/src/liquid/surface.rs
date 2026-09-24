use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::primitives::MeshAabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::mesh::{
    Indices, MeshTag, MeshVertexAttribute, MeshVertexBufferLayoutRef, PrimitiveTopology,
};
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Buffer, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use terrain::{LiquidKind, LiquidMesh};

use super::query::{LiquidSource, WmoPool, world_grid};
use super::{FoamPatch, frames};
use crate::Install;
use crate::coords::wow_to_bevy;
use crate::light::LightBuffer;
use crate::portal::WmoGroupVis;
use crate::sky_order::WATER_SORT_RUNG;
use crate::wmo::WmoRooms;

pub type LiquidMaterial = ExtendedMaterial<StandardMaterial, LiquidExtension>;

const WATER_SHININESS: f32 = 6.0;
const WMO_SCROLLING_MAGMA_NIBBLE: u8 = 6;
const WMO_SCROLLING_SLIME_NIBBLE: u8 = 7;
const DEPTH_COORD: MeshVertexAttribute = Mesh::ATTRIBUTE_UV_1;
const BODY_COLOR: MeshVertexAttribute = Mesh::ATTRIBUTE_COLOR;

/// `liquid.wgsl`'s `LiquidParams`, member for member.
#[derive(Clone, Copy, Default, Debug, ShaderType)]
pub struct LiquidParams {
    pub fullbright: f32,
    pub ocean: f32,
    pub room_fogged: f32,
    pub shininess: f32,
    pub renderer: f32,
    pub frame_count: f32,
    pub scrolls: f32,
}

#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct LiquidExtension {
    #[texture(100, dimension = "2d_array", visibility(fragment))]
    #[sampler(101, visibility(fragment))]
    pub frames: Handle<Image>,
    #[uniform(102)]
    pub params: LiquidParams,
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

    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        crate::sky_order::sort_only(descriptor);
        Ok(())
    }
}

pub(super) fn plugin(app: &mut App) {
    embedded_asset!(app, "liquid.wgsl");
    app.add_plugins(MaterialPlugin::<LiquidMaterial>::default());
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum LiquidRenderer {
    Adt,
    WmoExterior,
    WmoInterior,
}

impl LiquidRenderer {
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

pub(super) fn scrolls(nibble: u8) -> bool {
    matches!(
        nibble,
        WMO_SCROLLING_MAGMA_NIBBLE | WMO_SCROLLING_SLIME_NIBBLE
    )
}

struct FrameSet {
    kind: LiquidKind,
    dir: &'static str,
    stem: &'static str,
    frames_on_disk: u32,
}

const FRAME_SETS: &[FrameSet] = &[
    FrameSet {
        kind: LiquidKind::Still,
        dir: "river",
        stem: "lake_a",
        frames_on_disk: 30,
    },
    FrameSet {
        kind: LiquidKind::Rapids,
        dir: "river",
        stem: "fast_a",
        frames_on_disk: 16,
    },
    FrameSet {
        kind: LiquidKind::Ocean,
        dir: "ocean",
        stem: "ocean_h",
        frames_on_disk: 30,
    },
    FrameSet {
        kind: LiquidKind::Magma,
        dir: "lava",
        stem: "lava",
        frames_on_disk: 30,
    },
    FrameSet {
        kind: LiquidKind::Slime,
        dir: "slime",
        stem: "slime",
        frames_on_disk: 30,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct LiquidKey {
    kind: LiquidKind,
    renderer: LiquidRenderer,
    scroll: bool,
}

#[derive(Resource, Default)]
pub(crate) struct LiquidAssets {
    materials: HashMap<LiquidKey, Handle<LiquidMaterial>>,
}

impl LiquidAssets {
    fn material(&self, key: LiquidKey) -> Option<Handle<LiquidMaterial>> {
        self.materials.get(&key).cloned()
    }
}

pub(super) fn setup_liquid(
    mut commands: Commands<'_, '_>,
    install: Res<'_, Install>,
    light: Option<Res<'_, LightBuffer>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut materials: ResMut<'_, Assets<LiquidMaterial>>,
) {
    let Some(light) = light else {
        return;
    };
    let mut assets = LiquidAssets::default();
    for set in FRAME_SETS {
        let kind = set.kind;
        let decoded = frames::read_frames(&install.0, set.dir, set.stem, set.frames_on_disk);
        let Some(array) = frames::frame_array(&decoded, !kind.is_fullbright()) else {
            warn!("no frames for {}: {kind:?} will not draw", set.stem);
            continue;
        };
        let frames = images.add(array);
        let (alpha_mode, depth_bias) = if kind.is_fullbright() {
            (AlphaMode::Opaque, 0.0)
        } else {
            (AlphaMode::Blend, WATER_SORT_RUNG)
        };
        let scroll_lanes: &[bool] = if kind.is_fullbright() {
            &[false, true]
        } else {
            &[false]
        };
        for renderer in [
            LiquidRenderer::Adt,
            LiquidRenderer::WmoExterior,
            LiquidRenderer::WmoInterior,
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
                        params: LiquidParams {
                            fullbright: flag(kind.is_fullbright()),
                            ocean: flag(kind == LiquidKind::Ocean),
                            room_fogged: flag(renderer == LiquidRenderer::WmoInterior),
                            shininess: WATER_SHININESS,
                            renderer: renderer.shader_id(),
                            frame_count: decoded.len() as f32,
                            scrolls: flag(scroll),
                        },
                        light: light.0.clone(),
                    },
                });
                let key = LiquidKey {
                    kind,
                    renderer,
                    scroll,
                };
                assets.materials.insert(key, material);
            }
        }
    }
    commands.insert_resource(assets);
}

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
    mesh.insert_attribute(DEPTH_COORD, depth);
    if let Some([r, g, b]) = body_color {
        mesh.insert_attribute(BODY_COLOR, vec![[r, g, b, 1.0]; n]);
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
        world_grid(lq, &transform, source),
    ));
    if !lq.kind.is_fullbright() {
        e.insert(FoamPatch);
    }
    e.id()
}

pub(crate) fn spawn_adt_liquids<'a>(
    commands: &mut Commands<'_, '_>,
    meshes: &mut Assets<Mesh>,
    assets: &LiquidAssets,
    liquids: impl Iterator<Item = &'a LiquidMesh>,
) -> Vec<Entity> {
    liquids
        .filter_map(|lq| {
            let material = assets.material(LiquidKey {
                kind: lq.kind,
                renderer: LiquidRenderer::Adt,
                scroll: false,
            })?;
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
        let renderer = LiquidRenderer::wmo(nav.is_some_and(|g| g.interior));
        let key = LiquidKey {
            kind: lq.kind,
            renderer,
            scroll: scrolls(lq.sound_nibble),
        };
        let Some(material) = assets.material(key) else {
            continue;
        };
        let body_color = (renderer == LiquidRenderer::WmoInterior && !lq.kind.is_fullbright())
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
        let Some(bevy::mesh::VertexAttributeValues::Float32x2(depth)) = mesh.attribute(DEPTH_COORD)
        else {
            panic!("the depth coordinate");
        };
        assert_eq!(depth[2], [0.75, 0.0]);
        assert!(mesh.attribute(BODY_COLOR).is_none());
        let mesh = liquid_mesh(&lq, Some([0.1, 0.2, 0.3]));
        assert!(mesh.attribute(BODY_COLOR).is_some());
    }
}
