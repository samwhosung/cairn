use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};

use crate::view::FARCLIP;

/// Rows of four floats ahead of the point-light table, the layout the world shaders declare.
const HEADER_ROWS: usize = 21;
const MAX_POINT_LIGHTS: usize = 256;
const POINT_ROWS: usize = 2 * MAX_POINT_LIGHTS;

const AMBIENT: usize = 0;
const DIFFUSE: usize = 1;
const SUN: usize = 2;
const SPECULAR: usize = 3;
const FOG_COLOR: usize = 4;
const FOG_PARAMS: usize = 5;

/// The client's specular exponent for terrain.
const TERRAIN_SHININESS: f32 = 20.0;
const FOG_ON: f32 = 1.0;

/// The light at the camera, as the shaders take it. Colours are gamma-space `0..=1`.
#[derive(Resource, Clone, Copy, Default, PartialEq, Debug)]
pub struct SceneLight {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub specular: [f32; 3],
    /// The direction sunlight travels, in Bevy's axes.
    pub sun: Vec3,
    pub fog_color: [f32; 3],
    /// Yards; negative in a storm, which fogs the camera's own spot.
    pub fog_start: f32,
    pub fog_end: f32,
}

/// The storage buffer the world materials bind. It is rewritten in place each frame, so no
/// material or bind group changes when the light does.
#[derive(Resource, Clone, ExtractResource)]
pub struct LightBuffer(pub Buffer);

#[derive(Resource, Clone, Copy, Default, PartialEq, ExtractResource)]
struct LightRows([[f32; 4]; HEADER_ROWS]);

pub(crate) struct LightBufferPlugin;

impl Plugin for LightBufferPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneLight>()
            .init_resource::<LightRows>()
            .add_plugins((
                ExtractResourcePlugin::<LightRows>::default(),
                ExtractResourcePlugin::<LightBuffer>::default(),
            ))
            .add_systems(PostUpdate, pack_rows);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
        }
    }

    fn finish(&self, app: &mut App) {
        let Some(device) = app.world().get_resource::<RenderDevice>() else {
            return;
        };
        let size = ((HEADER_ROWS + POINT_ROWS) * 16) as u64;
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("scene_light"),
            size,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        app.insert_resource(LightBuffer(buffer));
    }
}

pub(crate) fn rows(light: &SceneLight) -> [[f32; 4]; HEADER_ROWS] {
    let rgb = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
    let mut rows = [[0.0; 4]; HEADER_ROWS];
    rows[AMBIENT] = rgb(light.ambient, 0.0);
    rows[DIFFUSE] = rgb(light.diffuse, 0.0);
    rows[SUN] = light.sun.extend(0.0).to_array();
    rows[SPECULAR] = rgb(light.specular, TERRAIN_SHININESS);
    rows[FOG_COLOR] = rgb(light.fog_color, FOG_ON);
    rows[FOG_PARAMS] = [light.fog_start, light.fog_end, 0.0, FARCLIP];
    rows
}

fn pack_rows(light: Res<'_, SceneLight>, mut packed: ResMut<'_, LightRows>) {
    let fresh = LightRows(rows(&light));
    if *packed != fresh {
        *packed = fresh;
    }
}

fn upload(
    queue: Res<'_, RenderQueue>,
    buffer: Option<Res<'_, LightBuffer>>,
    rows: Res<'_, LightRows>,
) {
    let Some(buffer) = buffer else {
        return;
    };
    let bytes: Vec<u8> = rows
        .0
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    queue.write_buffer(&buffer.0, 0, &bytes);
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn rows_land_where_the_shaders_read_them() {
        let light = SceneLight {
            ambient: [0.1, 0.2, 0.3],
            diffuse: [0.4, 0.5, 0.6],
            specular: [0.7, 0.8, 0.9],
            sun: Vec3::new(0.5, -0.5, 0.25),
            fog_color: [0.3, 0.4, 0.5],
            fog_start: 87.5,
            fog_end: 350.0,
        };
        let rows = rows(&light);
        assert_eq!(rows[0], [0.1, 0.2, 0.3, 0.0]);
        assert_eq!(rows[1], [0.4, 0.5, 0.6, 0.0]);
        assert_eq!(rows[2], [0.5, -0.5, 0.25, 0.0]);
        assert_eq!(rows[3], [0.7, 0.8, 0.9, 20.0]);
        assert_eq!(rows[4], [0.3, 0.4, 0.5, 1.0]);
        assert_eq!(rows[5], [87.5, 350.0, 0.0, 350.0]);
        assert!(rows[6..].iter().flatten().all(|&v| v == 0.0));
    }
}
