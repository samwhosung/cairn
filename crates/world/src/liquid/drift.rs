//! The motes drifting around the eye while it is under water or magma: 4000 of them in a 30-yard
//! box around the camera, fixed in the world but for a slow gust, wrapped back into the box as it
//! moves and scattered afresh only when the liquid changes or the camera jumps.

use bevy::asset::RenderAssetUsages;
use bevy::camera::Projection;
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use light::Submersion;

use super::Underwater;
use crate::effect::{EffectLook, EffectMaterial, effect_material};
use crate::light::LightBuffer;
use crate::source::{Repeat, texture_url};
use crate::view::WorldCamera;

const COUNT: usize = 4000;
const BOX_EDGE: f32 = 30.0;
const BOX_HALF: f32 = BOX_EDGE * 0.5;
/// A camera step this long in one frame scatters the field instead of carrying it.
const TELEPORT: f32 = BOX_EDGE;
/// Mote edge, the middle of its `[½, 1½)` spread, yards.
const SCALE_WATER: f32 = 1.0 / 36.0;
const SCALE_MAGMA: f32 = 1.0 / 9.0;
const GUST_FREQ_UNIT: f32 = 0.0125;
const GUST_AMP_UNIT: f32 = 0.005;
/// Squashes a new gust's rise before it is normalised; it never blows down.
const GUST_RISE: f32 = 0.25;
/// Magma's motes sink this fast, yards a second.
const MAGMA_SINK: f32 = -0.02;
/// The water gust is a displacement per frame in the client; this is the frame rate it is read
/// at, so the drift keeps its speed at any rate.
const GUST_REF_HZ: f32 = 60.0;
const SUBMIT_CAP: usize = 666;
/// After the water surface and every model transparent: the last world content drawn.
const DRIFT_SORT: f32 = 1.4e4;
const CELL: f32 = 51.0 / 256.0;
/// The atlas cells, `(column, row)`.
const ATLAS: [(f32, f32); 13] = [
    (0.0, 0.0),
    (1.0, 0.0),
    (2.0, 0.0),
    (3.0, 0.0),
    (0.0, 1.0),
    (1.0, 1.0),
    (2.0, 1.0),
    (3.0, 1.0),
    (4.0, 0.0),
    (0.0, 2.0),
    (1.0, 2.0),
    (2.0, 2.0),
    (3.0, 2.0),
];
const CELLS_WATER: [usize; 8] = [0, 1, 2, 3, 4, 5, 6, 7];
const CELLS_MAGMA: [usize; 8] = [9, 10, 11, 12, 9, 10, 11, 12];
const TEXTURE: &str = "Textures\\WaterPoop02.blp";

/// A 24-bit uniform draw in `[0, 1)` from a xorshift32.
pub(crate) fn rand01(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The half-angle tangents motes are kept within: a 90-degree cone around the view, or the
/// frustum where a wide window reaches past it.
fn cull_limits(fov_y: f32, aspect: f32) -> (f32, f32) {
    let ty = (fov_y * 0.5).tan();
    ((ty * aspect).max(1.0), ty.max(1.0))
}

#[derive(Clone, Copy, Default)]
struct Mote {
    /// Relative to the camera.
    pos: Vec3,
    edge: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DriftMode {
    Water,
    Magma,
}

impl DriftMode {
    fn scale_base(self) -> f32 {
        match self {
            DriftMode::Water => SCALE_WATER,
            DriftMode::Magma => SCALE_MAGMA,
        }
    }

    fn cells(self) -> &'static [usize; 8] {
        match self {
            DriftMode::Water => &CELLS_WATER,
            DriftMode::Magma => &CELLS_MAGMA,
        }
    }
}

#[derive(Resource)]
struct DriftCloud {
    motes: Vec<Mote>,
    last_cam: Vec3,
    /// `None` until a liquid configures it, and in slime.
    mode: Option<DriftMode>,
    was: Submersion,
    gust_dir: Vec3,
    gust_freq: f32,
    gust_phase: f32,
    gust_amp: f32,
    rng: u32,
}

impl Default for DriftCloud {
    fn default() -> Self {
        Self {
            motes: vec![Mote::default(); COUNT],
            last_cam: Vec3::ZERO,
            mode: None,
            was: Submersion::Dry,
            gust_dir: Vec3::X,
            gust_freq: GUST_FREQ_UNIT,
            gust_phase: 0.0,
            gust_amp: GUST_AMP_UNIT,
            rng: 0x9e37_79b9,
        }
    }
}

impl DriftCloud {
    fn scatter(&mut self, mode: DriftMode) {
        let base = mode.scale_base();
        let lo = base * 0.5;
        let span = base * 1.5 - lo;
        let mut rng = self.rng;
        for m in &mut self.motes {
            m.pos = Vec3::new(
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
                rand01(&mut rng) * BOX_EDGE - BOX_HALF,
            );
            m.edge = rand01(&mut rng) * span + lo;
        }
        self.rng = rng;
        self.mode = Some(mode);
    }

    fn roll_gust(&mut self) {
        let mut rng = self.rng;
        let angle = |r: &mut u32| (rand01(r) * 2.0 - 1.0) * std::f32::consts::PI;
        let (sa, ca) = angle(&mut rng).sin_cos();
        let (se, ce) = angle(&mut rng).sin_cos();
        let dir = Vec3::new(ce * sa, (ca * GUST_RISE).abs(), se * sa);
        self.gust_dir = dir.normalize_or(Vec3::Y);
        self.gust_freq = (1.0 + rand01(&mut rng)) * GUST_FREQ_UNIT;
        self.gust_amp = (1.0 + rand01(&mut rng)) * GUST_AMP_UNIT;
        self.gust_phase = 0.0;
        self.rng = rng;
    }

    /// The whole field's step this frame: a half-sine gust swelling and ebbing over 20 to 40
    /// seconds in water, a steady sink in magma.
    fn gust(&mut self, mode: DriftMode, dt: f32) -> Vec3 {
        match mode {
            DriftMode::Magma => Vec3::new(0.0, MAGMA_SINK * dt, 0.0),
            DriftMode::Water => {
                self.gust_phase += dt;
                let mut term = self.gust_phase * self.gust_freq;
                if term > 0.5 {
                    self.roll_gust();
                    term = 0.0;
                }
                let speed = (term * std::f32::consts::TAU).sin() * self.gust_amp;
                self.gust_dir * (speed * dt * GUST_REF_HZ)
            }
        }
    }

    fn advect(&mut self, mode: DriftMode, eye: Vec3, dt: f32) {
        let mut delta = self.last_cam - eye;
        self.last_cam = eye;
        if delta.length_squared() > TELEPORT * TELEPORT {
            self.scatter(mode);
            delta = Vec3::ZERO;
        }
        let add = delta + self.gust(mode, dt);
        let wrap = |v: f32| {
            if v > BOX_HALF {
                v - BOX_EDGE
            } else if v < -BOX_HALF {
                v + BOX_EDGE
            } else {
                v
            }
        };
        for m in &mut self.motes {
            let moved = m.pos + add;
            m.pos = Vec3::new(wrap(moved.x), wrap(moved.y), wrap(moved.z));
        }
    }
}

/// The drawn field: one mesh of camera-relative quads, and its two looks.
#[derive(Resource)]
struct DriftDraw {
    entity: Entity,
    mesh: Handle<Mesh>,
    water: Handle<EffectMaterial>,
    magma: Handle<EffectMaterial>,
}

fn setup_drift(
    mut commands: Commands<'_, '_>,
    server: Res<'_, AssetServer>,
    light: Option<Res<'_, LightBuffer>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<EffectMaterial>>,
) {
    let Some(light) = light else {
        return;
    };
    let texture = server.load(texture_url(TEXTURE, Repeat { u: false, v: false }));
    let look = |fogged| EffectLook {
        additive: false,
        fogged,
        camera_relative: true,
        sort: DRIFT_SORT,
        raster_bias: 0,
    };
    let water = materials.add(effect_material(look(false), texture.clone(), &light.0));
    let magma = materials.add(effect_material(look(true), texture, &light.0));
    let mesh = meshes.add(quad_mesh(vec![[0.0; 3]; 4], vec![[0.0; 2]; 4]));
    let entity = commands
        .spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(water.clone()),
            Transform::default(),
            Visibility::Hidden,
            NoFrustumCulling,
        ))
        .id();
    commands.insert_resource(DriftDraw {
        entity,
        mesh,
        water,
        magma,
    });
}

/// A new liquid scatters the field; leaving liquid keeps it, and slime has none.
fn simulate_drift(
    mut cloud: ResMut<'_, DriftCloud>,
    underwater: Res<'_, Underwater>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
    time: Res<'_, Time>,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let eye = cam.translation;
    let now = underwater.0;
    if now != cloud.was {
        cloud.was = now;
        match now {
            Submersion::Dry => {}
            Submersion::Slime => cloud.mode = None,
            Submersion::Water | Submersion::Ocean => {
                cloud.scatter(DriftMode::Water);
                cloud.last_cam = eye;
            }
            Submersion::Magma => {
                cloud.scatter(DriftMode::Magma);
                cloud.last_cam = eye;
            }
        }
    }
    if !now.any() {
        return;
    }
    let Some(mode) = cloud.mode else {
        return;
    };
    cloud.advect(mode, eye, time.delta_secs());
}

/// Up to [`SUBMIT_CAP`] motes inside the view's cone, each a camera-facing quad of its cell of
/// the atlas, white.
fn quads(cloud: &DriftCloud, mode: DriftMode, cam: &Transform, tan: (f32, f32)) -> Mesh {
    let (fwd, right, up) = (*cam.forward(), *cam.right(), *cam.up());
    let cells = mode.cells();
    let (mut positions, mut uvs) = (Vec::new(), Vec::new());
    for (i, m) in cloud.motes.iter().enumerate() {
        if positions.len() == SUBMIT_CAP * 4 {
            break;
        }
        let rel = m.pos;
        let vz = rel.dot(fwd);
        if vz <= 0.0 || rel.dot(right).abs() >= vz * tan.0 || rel.dot(up).abs() >= vz * tan.1 {
            continue;
        }
        let (col, row) = ATLAS[cells[i & 7]];
        let (u0, v0) = (col * CELL, row * CELL);
        let (r, u) = (right * (m.edge * 0.5), up * (m.edge * 0.5));
        for (p, uv) in [
            (rel - r - u, [u0, v0 + CELL]),
            (rel + r - u, [u0 + CELL, v0 + CELL]),
            (rel + r + u, [u0 + CELL, v0]),
            (rel - r + u, [u0, v0]),
        ] {
            positions.push(p.to_array());
            uvs.push(uv);
        }
    }
    quad_mesh(positions, uvs)
}

/// White quads, four corners each in perimeter order.
fn quad_mesh(positions: Vec<[f32; 3]>, uvs: Vec<[f32; 2]>) -> Mesh {
    let n = positions.len() as u32;
    let indices = (0..n / 4)
        .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|k| q * 4 + k))
        .collect();
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, vec![[1.0f32; 4]; n as usize]);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[allow(clippy::type_complexity)]
fn draw_drift(
    cloud: Res<'_, DriftCloud>,
    underwater: Res<'_, Underwater>,
    draw: Option<Res<'_, DriftDraw>>,
    camera: Query<'_, '_, (&Transform, &Projection), With<WorldCamera>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut field: Query<
        '_,
        '_,
        (
            &mut Transform,
            &mut Visibility,
            &mut MeshMaterial3d<EffectMaterial>,
        ),
        Without<WorldCamera>,
    >,
) {
    let (Some(draw), Ok((cam, projection))) = (draw, camera.single()) else {
        return;
    };
    let Ok((mut at, mut vis, mut material)) = field.get_mut(draw.entity) else {
        return;
    };
    let mode = cloud.mode.filter(|_| underwater.0.any());
    let Some(mode) = mode else {
        vis.set_if_neq(Visibility::Hidden);
        return;
    };
    let tan = match projection {
        Projection::Perspective(p) => cull_limits(p.fov, p.aspect_ratio),
        _ => (1.0, 1.0),
    };
    let field = quads(&cloud, mode, cam, tan);
    if field.count_vertices() == 0 {
        vis.set_if_neq(Visibility::Hidden);
        return;
    }
    if let Some(mesh) = meshes.get_mut(&draw.mesh) {
        *mesh = field;
    }
    at.translation = cam.translation;
    let want = match mode {
        DriftMode::Water => &draw.water,
        DriftMode::Magma => &draw.magma,
    };
    if material.0 != *want {
        material.0 = want.clone();
    }
    vis.set_if_neq(Visibility::Inherited);
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DriftCloud>()
        .add_systems(Startup, setup_drift)
        .add_systems(Update, simulate_drift.after(super::SubmersionVerdict))
        .add_systems(
            PostUpdate,
            draw_drift.before(bevy::transform::TransformSystems::Propagate),
        );
}

#[cfg(test)]
mod tests;
