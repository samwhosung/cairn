use std::collections::BTreeMap;

use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Buffer};
use bevy::shader::ShaderRef;
use wdl::WdlFile;

use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::light::LightBuffer;
use crate::view::WorldCamera;
use crate::{CurrentMap, Install, Residency};

/// Tiles out from the camera's: 5 × 533 yards reaches just short of the projection's far plane.
const RING_RADIUS_TILES: u32 = 5;

pub type WdlMaterial = ExtendedMaterial<StandardMaterial, WdlExtension>;

#[derive(Asset, AsBindGroup, Clone, TypePath)]
pub struct WdlExtension {
    #[storage(90, read_only, buffer)]
    pub light: Buffer,
}

impl MaterialExtension for WdlExtension {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/wdl.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/wdl.wgsl".into()
    }
}

pub(crate) struct HorizonPlugin;

impl Plugin for HorizonPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "wdl.wgsl");
        app.add_plugins(MaterialPlugin::<WdlMaterial>::default())
            .init_resource::<Horizon>();
    }
}

#[derive(Default)]
enum Wdl {
    #[default]
    Unread,
    Missing,
    Read(WdlFile),
}

#[derive(Resource, Default)]
pub(crate) struct Horizon {
    wdl: Wdl,
    material: Option<Handle<WdlMaterial>>,
    spawned: BTreeMap<(u32, u32), Entity>,
}

impl Horizon {
    pub(crate) fn height_under(&self, bevy_pos: Vec3) -> Option<f32> {
        let Wdl::Read(wdl) = &self.wdl else {
            return None;
        };
        let [x, y, _] = bevy_to_wow(bevy_pos);
        wdl.height_at(x, y)
    }
}

fn read_wdl(install: &Install, dir: &str) -> Wdl {
    let path = format!("World\\Maps\\{dir}\\{dir}.wdl");
    let parsed = install
        .0
        .read(&path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| WdlFile::parse(&bytes).map_err(|e| format!("{path}: {e}")));
    match parsed {
        Ok(wdl) => Wdl::Read(wdl),
        Err(e) => {
            warn!("no horizon: {e}");
            Wdl::Missing
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn stream_horizon(
    mut commands: Commands<'_, '_>,
    install: Res<'_, Install>,
    map: Res<'_, CurrentMap>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
    light: Option<Res<'_, LightBuffer>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<WdlMaterial>>,
    mut horizon: ResMut<'_, Horizon>,
    mut residency: ResMut<'_, Residency>,
) {
    let (Ok(camera), Some(light)) = (camera.single(), light) else {
        return;
    };
    let horizon = &mut *horizon;
    if matches!(horizon.wdl, Wdl::Unread) {
        horizon.wdl = read_wdl(&install, &map.directory);
    }
    residency.horizon = true;
    let Wdl::Read(wdl) = &horizon.wdl else {
        return;
    };
    let material = horizon
        .material
        .get_or_insert_with(|| {
            materials.add(ExtendedMaterial {
                base: StandardMaterial {
                    base_color: Color::WHITE,
                    unlit: true,
                    cull_mode: None,
                    ..StandardMaterial::default()
                },
                extension: WdlExtension {
                    light: light.0.clone(),
                },
            })
        })
        .clone();
    let [x, y, _] = bevy_to_wow(camera.translation);
    let wanted = wdl.tiles_around(x, y, RING_RADIUS_TILES);
    horizon.spawned.retain(|tile, entity| {
        let keep = wanted.contains(tile);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    for (tx, ty) in wanted {
        if horizon.spawned.contains_key(&(tx, ty)) {
            continue;
        }
        let Some(tile) = wdl.tile_mesh(tx, ty) else {
            continue;
        };
        let positions: Vec<[f32; 3]> = tile
            .positions
            .iter()
            .map(|p| wow_to_bevy(*p).to_array())
            .collect();
        let n = positions.len();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 1.0, 0.0]; n]);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[0.0, 0.0]; n]);
        mesh.insert_indices(Indices::U32(tile.indices));
        let entity = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                MeshMaterial3d(material.clone()),
                Transform::IDENTITY,
            ))
            .id();
        horizon.spawned.insert((tx, ty), entity);
    }
}
