use bevy::camera::Projection;
use bevy::prelude::*;
use light::Submersion;

use super::Underwater;
use crate::effects::{
    EffectBlend, EffectDrawSpec, EffectFog, EffectLighting, EffectQuads, EffectVertex,
    begin_effect_frame,
};
use crate::sky_order::DRIFT_SORT_RUNG;
use crate::source::{Repeat, texture_url};
use crate::view::WorldCamera;

const COUNT: usize = 4000;
const BOX_EDGE: f32 = 30.0;
const BOX_HALF: f32 = BOX_EDGE * 0.5;
const SCATTER_STEP: f32 = BOX_EDGE;
const EDGE_WATER: f32 = 1.0 / 36.0;
const EDGE_MAGMA: f32 = 1.0 / 9.0;
const GUST_FREQ_UNIT: f32 = 0.5 / 40.0;
const GUST_AMP_UNIT: f32 = 0.005;
const GUST_RISE: f32 = 0.25;
const MAGMA_SINK_SPEED: f32 = -0.02;
/// The water gust is a displacement per frame in the client; this is the frame rate it is read
/// at, so the drift keeps its speed at any rate.
const GUST_REF_HZ: f32 = 60.0;
const SUBMIT_CAP: usize = 666;
const CELL: f32 = 51.0 / 256.0;
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

pub(crate) fn rand01(state: &mut u32) -> f32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// The client keeps motes within a right-angled cone, or the frustum where that is wider.
const MIN_HALF_ANGLE_TAN: f32 = 1.0;

#[derive(Clone, Copy, PartialEq, Debug)]
struct ViewCone {
    right_tan: f32,
    up_tan: f32,
}

fn cull_limits(fov_y: f32, aspect: f32) -> ViewCone {
    let up = (fov_y * 0.5).tan();
    ViewCone {
        right_tan: (up * aspect).max(MIN_HALF_ANGLE_TAN),
        up_tan: up.max(MIN_HALF_ANGLE_TAN),
    }
}

#[derive(Clone, Copy, Default)]
struct Mote {
    from_camera: Vec3,
    edge: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DriftMode {
    Water,
    Magma,
}

impl DriftMode {
    fn edge(self) -> f32 {
        match self {
            DriftMode::Water => EDGE_WATER,
            DriftMode::Magma => EDGE_MAGMA,
        }
    }

    fn cells(self) -> &'static [usize; 8] {
        match self {
            DriftMode::Water => &CELLS_WATER,
            DriftMode::Magma => &CELLS_MAGMA,
        }
    }

    fn fog(self) -> EffectFog {
        match self {
            DriftMode::Water => EffectFog::Off,
            DriftMode::Magma => EffectFog::Scene,
        }
    }
}

#[derive(Resource)]
struct DriftCloud {
    motes: Vec<Mote>,
    last_cam: Vec3,
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
        let base = mode.edge();
        let lo = base * 0.5;
        let span = base * 1.5 - lo;
        let mut rng = self.rng;
        for m in &mut self.motes {
            m.from_camera = Vec3::new(
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

    fn gust(&mut self, mode: DriftMode, dt: f32) -> Vec3 {
        match mode {
            DriftMode::Magma => Vec3::new(0.0, MAGMA_SINK_SPEED * dt, 0.0),
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
        if delta.length_squared() > SCATTER_STEP * SCATTER_STEP {
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
            let moved = m.from_camera + add;
            m.from_camera = Vec3::new(wrap(moved.x), wrap(moved.y), wrap(moved.z));
        }
    }
}

#[derive(Resource)]
struct DriftTexture(Handle<Image>);

fn setup_drift(mut commands: Commands<'_, '_>, server: Res<'_, AssetServer>) {
    let texture = server.load(texture_url(TEXTURE, Repeat { u: false, v: false }));
    commands.insert_resource(DriftTexture(texture));
}

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

struct CameraAxes {
    forward: Vec3,
    right: Vec3,
    up: Vec3,
}

fn push_quads(
    cloud: &DriftCloud,
    mode: DriftMode,
    cam: &CameraAxes,
    cone: ViewCone,
    verts: &mut Vec<EffectVertex>,
) {
    let cells = mode.cells();
    let mut submitted = 0;
    for (i, m) in cloud.motes.iter().enumerate() {
        if submitted == SUBMIT_CAP {
            break;
        }
        let rel = m.from_camera;
        let vz = rel.dot(cam.forward);
        if vz <= 0.0
            || rel.dot(cam.right).abs() >= vz * cone.right_tan
            || rel.dot(cam.up).abs() >= vz * cone.up_tan
        {
            continue;
        }
        let (col, row) = ATLAS[cells[i & 7]];
        let (u0, v0) = (col * CELL, row * CELL);
        let (r, u) = (cam.right * (m.edge * 0.5), cam.up * (m.edge * 0.5));
        for (pos, uv) in [
            (rel - r - u, [u0, v0 + CELL]),
            (rel + r - u, [u0 + CELL, v0 + CELL]),
            (rel + r + u, [u0 + CELL, v0]),
            (rel - r + u, [u0, v0]),
        ] {
            verts.push(EffectVertex {
                pos: pos.to_array(),
                uv,
                color: [1.0; 4],
            });
        }
        submitted += 1;
    }
}

fn push_drift(
    cloud: Res<'_, DriftCloud>,
    underwater: Res<'_, Underwater>,
    texture: Option<Res<'_, DriftTexture>>,
    camera: Query<'_, '_, (Entity, &GlobalTransform, &Projection), With<WorldCamera>>,
    mut effects: ResMut<'_, EffectQuads>,
) {
    let (Some(texture), Ok((cam, at, projection))) = (texture, camera.single()) else {
        return;
    };
    let Some(mode) = cloud.mode.filter(|_| underwater.0.any()) else {
        return;
    };
    let cone = match projection {
        Projection::Perspective(p) => cull_limits(p.fov, p.aspect_ratio),
        _ => ViewCone {
            right_tan: MIN_HALF_ANGLE_TAN,
            up_tan: MIN_HALF_ANGLE_TAN,
        },
    };
    let basis = CameraAxes {
        forward: *at.forward(),
        right: *at.right(),
        up: *at.up(),
    };
    let start = effects.begin();
    push_quads(&cloud, mode, &basis, cone, &mut effects.verts);
    effects.commit_quads(
        start,
        EffectDrawSpec {
            cam,
            texture: texture.0.id(),
            blend: EffectBlend::Alpha,
            fog: mode.fog(),
            lighting: EffectLighting::None,
            sort_anchor: at.translation(),
            sort_bias: DRIFT_SORT_RUNG,
            raster_bias: 0,
            raster_slope: 0.0,
            cam_relative: true,
            no_depth_test: false,
            main_entity: Entity::PLACEHOLDER,
        },
    );
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DriftCloud>()
        .add_systems(Startup, setup_drift)
        .add_systems(Update, simulate_drift.after(super::SubmersionVerdict))
        .add_systems(PostUpdate, push_drift.after(begin_effect_frame));
}

#[cfg(test)]
mod tests;
