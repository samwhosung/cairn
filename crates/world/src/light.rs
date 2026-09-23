use bevy::prelude::*;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_resource::{Buffer, BufferDescriptor, BufferUsages};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::transform::TransformSystems;

use crate::portal::{WmoGroupVis, WmoPortalInstance, room_admits};
use crate::probes::{MAX_PROP_PROBES, PROBE_ROWS};
use crate::rig::{BONE_BYTES, MAX_PALETTE_BONES, MAX_RIG_SLOTS};
use crate::sh::sh_probe_coeffs;
use crate::view::{FARCLIP, WorldCamera};

/// Rows of four floats ahead of the point-light table, the layout the world shaders declare.
const HEADER_ROWS: usize = 21;
const MAX_POINT_LIGHTS: usize = 256;
const POINT_ROWS: usize = 2 * MAX_POINT_LIGHTS;
const ROW_BYTES: usize = 16;
pub(crate) const PROBE_REGION_OFFSET: u64 = ((HEADER_ROWS + POINT_ROWS) * ROW_BYTES) as u64;
/// Mirrors `WowLight` in model.wgsl.
pub(crate) const RIG_TABLE_OFFSET: u64 =
    PROBE_REGION_OFFSET + (MAX_PROP_PROBES * PROBE_ROWS * ROW_BYTES) as u64;
const TINT_REGION_OFFSET: u64 = RIG_TABLE_OFFSET + (MAX_RIG_SLOTS * 4) as u64;
pub(crate) const RIG_ORIGIN_OFFSET: u64 = TINT_REGION_OFFSET + (MAX_RIG_SLOTS * 4) as u64;
const MATANIM_ROWS: usize = 2048;
const MATANIM_OFFSET: u64 = RIG_ORIGIN_OFFSET + (MAX_RIG_SLOTS * ROW_BYTES) as u64;
pub(crate) const RIG_PALETTE_OFFSET: u64 = MATANIM_OFFSET + (MATANIM_ROWS * ROW_BYTES) as u64;
const BUFFER_BYTES: u64 = RIG_PALETTE_OFFSET + MAX_PALETTE_BONES as u64 * BONE_BYTES;

const AMBIENT: usize = 0;
const DIFFUSE: usize = 1;
const SUN: usize = 2;
const SPECULAR: usize = 3;
const FOG_COLOR: usize = 4;
const FOG_PARAMS: usize = 5;
const SH_FIRST: usize = 6;
const SH_C16: usize = 12;
const GRADE: usize = 17;
const WMO_FOG_COLOR: usize = 18;
const WMO_FOG_PARAMS: usize = 19;
const POINT_COUNT: usize = 20;

/// The client's specular exponent for terrain.
const TERRAIN_SHININESS: f32 = 20.0;
const ON: f32 = 1.0;

const POINT_PACK_RADIUS: f32 = 300.0;
const POINT_LIGHT_RANGE: f32 = 48.0;

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
    /// The sky dome's colours, zenith first.
    pub sky: [[f32; 3]; 5],
    /// How strongly dawn and dusk warp the dome, `0..=1`.
    pub sky_warp: f32,
    /// The direction to the visible sun, in Bevy's axes.
    pub visible_sun: Vec3,
    /// How much of a window's night glow shows: 1 overnight, 0 by day.
    pub night_glow: f32,
}

#[derive(Component, Clone, Copy, Debug, PartialEq)]
pub(crate) struct WorldPointLight {
    color: [f32; 3],
    intensity: f32,
    range: f32,
}

pub(crate) fn point_light(color: [f32; 3], intensity: f32) -> WorldPointLight {
    WorldPointLight {
        color,
        intensity: intensity.max(0.0),
        range: POINT_LIGHT_RANGE,
    }
}

#[derive(Component)]
pub(crate) struct LightRooms(pub WmoGroupVis);

/// The storage buffer the world materials bind. It is rewritten in place each frame, so no
/// material or bind group changes when the light does.
#[derive(Resource, Clone, ExtractResource)]
pub struct LightBuffer(pub Buffer);

#[derive(Resource, Clone, PartialEq, ExtractResource)]
struct LightRows {
    header: [[f32; 4]; HEADER_ROWS],
    points: Vec<[f32; 4]>,
}

impl Default for LightRows {
    fn default() -> Self {
        Self {
            header: [[0.0; 4]; HEADER_ROWS],
            points: Vec::new(),
        }
    }
}

pub(crate) struct LightBufferPlugin;

impl Plugin for LightBufferPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneLight>()
            .init_resource::<LightRows>()
            .add_plugins((
                ExtractResourcePlugin::<LightRows>::default(),
                ExtractResourcePlugin::<LightBuffer>::default(),
            ))
            .add_systems(PostUpdate, pack_rows.after(TransformSystems::Propagate));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, upload.in_set(RenderSystems::PrepareResources));
        }
    }

    fn finish(&self, app: &mut App) {
        let Some(device) = app.world().get_resource::<RenderDevice>() else {
            return;
        };
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("scene_light"),
            size: BUFFER_BYTES,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        app.insert_resource(LightBuffer(buffer));
    }
}

pub(crate) fn rows(light: &SceneLight) -> [[f32; 4]; HEADER_ROWS] {
    let rgb = |c: [f32; 3], w: f32| [c[0], c[1], c[2], w];
    let mut rows = [[0.0; 4]; HEADER_ROWS];
    rows[AMBIENT] = rgb(light.ambient, ON);
    rows[DIFFUSE] = rgb(light.diffuse, ON);
    rows[SUN] = light.sun.extend(ON).to_array();
    rows[SPECULAR] = rgb(light.specular, TERRAIN_SHININESS);
    rows[FOG_COLOR] = rgb(light.fog_color, ON);
    rows[FOG_PARAMS] = [light.fog_start, light.fog_end, 0.0, FARCLIP];
    let sun = sh_probe_coeffs([0.0; 3], &[(-light.sun, light.diffuse)]);
    for (i, row) in sun.iter().enumerate().take(6) {
        rows[SH_FIRST + i] = row.to_array();
    }
    for ch in 0..3 {
        rows[SH_FIRST + ch][3] = light.ambient[ch];
        rows[SH_C16][ch] = sun[6][ch];
        rows[GRADE][1 + ch] = sun[ch].w;
    }
    rows[GRADE][0] = light.night_glow;
    rows[WMO_FOG_COLOR] = rgb(light.fog_color, ON);
    rows[WMO_FOG_PARAMS] = [light.fog_start, light.fog_end, 0.0, 0.0];
    rows
}

type Lights<'w, 's> = Query<
    'w,
    's,
    (
        &'static WorldPointLight,
        &'static GlobalTransform,
        Option<&'static LightRooms>,
    ),
>;

fn pack_rows(
    light: Res<'_, SceneLight>,
    camera: Query<'_, '_, &GlobalTransform, With<WorldCamera>>,
    lights: Lights<'_, '_>,
    instances: Query<'_, '_, &WmoPortalInstance>,
    mut packed: ResMut<'_, LightRows>,
) {
    let cam_pos = camera
        .single()
        .map_or(Vec3::ZERO, GlobalTransform::translation);
    let mut pts: Vec<(f32, Vec3, f32, [f32; 3])> = lights
        .iter()
        .filter(|(_, _, rooms)| {
            room_admits(
                rooms.map(|r| &r.0),
                rooms.and_then(|r| instances.get(r.0.instance).ok()),
            )
        })
        .filter_map(|(pl, gt, _)| {
            let p = gt.translation();
            let d2 = p.distance_squared(cam_pos);
            (d2 < POINT_PACK_RADIUS * POINT_PACK_RADIUS).then(|| {
                let rgb = pl.color.map(|c| (c * pl.intensity).max(0.0));
                (d2, p, pl.range, rgb)
            })
        })
        .collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.truncate(MAX_POINT_LIGHTS);
    let mut header = rows(&light);
    header[POINT_COUNT] = [pts.len() as f32, 0.0, 0.0, 0.0];
    let points = pts
        .iter()
        .flat_map(|(_, p, range, rgb)| [[p.x, p.y, p.z, *range], [rgb[0], rgb[1], rgb[2], 0.0]])
        .collect();
    let fresh = LightRows { header, points };
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
        .header
        .iter()
        .chain(&rows.points)
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
            night_glow: 0.25,
            ..SceneLight::default()
        };
        let rows = rows(&light);
        assert_eq!(rows[0], [0.1, 0.2, 0.3, 1.0]);
        assert_eq!(rows[1], [0.4, 0.5, 0.6, 1.0]);
        assert_eq!(rows[2], [0.5, -0.5, 0.25, 1.0]);
        assert_eq!(rows[3], [0.7, 0.8, 0.9, 20.0]);
        assert_eq!(rows[4], [0.3, 0.4, 0.5, 1.0]);
        assert_eq!(rows[5], [87.5, 350.0, 0.0, 350.0]);
        assert_eq!([rows[6][3], rows[7][3], rows[8][3]], [0.1, 0.2, 0.3]);
        assert_eq!(rows[17][0], 0.25);
        assert_eq!(rows[18], rows[4]);
        assert_eq!(rows[19], [87.5, 350.0, 0.0, 0.0]);
        assert!(rows[13..17].iter().flatten().all(|&v| v == 0.0));
        assert_eq!(rows[20], [0.0; 4]);
    }

    #[test]
    fn the_sh_rows_light_a_model_as_the_closed_form() {
        let light = SceneLight {
            ambient: [0.30, 0.32, 0.38],
            diffuse: [0.85, 0.70, 0.45],
            sun: Vec3::new(0.3, -0.8, 0.52).normalize(),
            ..SceneLight::default()
        };
        let rows = rows(&light);
        let u = -light.sun;
        let side = u.cross(Vec3::Y).normalize();
        for intensity in [0.5f32, 1.0] {
            for n in [u, -u, side] {
                let quad = [n.x * n.y, n.y * n.z, n.z * n.z, n.x * n.z];
                let mu = n.dot(u);
                let f = (3.0 + 16.0 * mu + 15.0 * mu * mu) / 34.0;
                for ch in 0..3 {
                    let (c10, c13) = (rows[6 + ch], rows[9 + ch]);
                    let lin = c10[0] * n.x + c10[1] * n.y + c10[2] * n.z;
                    let q: f32 = (0..4).map(|k| c13[k] * quad[k]).sum::<f32>()
                        + rows[12][ch] * (n.x * n.x - n.y * n.y);
                    let got = c10[3] + rows[17][1 + ch] * intensity + intensity * (lin + q);
                    let want = light.ambient[ch] + light.diffuse[ch] * intensity * f;
                    assert!((got - want).abs() < 1e-5, "I={intensity} ch{ch} {n}");
                }
            }
        }
    }
}
