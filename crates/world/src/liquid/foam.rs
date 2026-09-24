mod params;

use std::collections::BTreeMap;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::drift::rand01;
use super::query::{CellCorners, LiquidGrid};
use super::{FoamPatch, WaterIndex};
use crate::Install;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::effects::{
    EffectBlend, EffectDrawSpec, EffectFog, EffectLighting, EffectQuads, EffectVertex,
    begin_effect_frame,
};
use crate::interior::Viewer;
use crate::sky_order::FOAM_SORT_RUNG;
use crate::view::WorldCamera;
use params::{
    RING_INTERVAL_SECS, WadeState, foam_params, foam_uv, record_alpha, record_size, wake_cooldown,
};

const RING_TEXTURE: &str = "XTextures\\splash\\splash.blp";
const WAKE_TEXTURE: &str = "XTextures\\splash\\wake.blp";
/// The avatar's share of the client's foam pool.
const POOL_SIZE: usize = 32;
const ONESHOT_DEPTH_FRAC: f32 = 0.4;
const GATE_DEPTH_FRAC: f32 = 2.0;
const MIN_GATE_DEPTH: f32 = 1.0;
/// A few steps of depth toward the eye, against a driver rounding the two coplanar draws apart.
const FOAM_RASTER: i32 = 8;

const AUTHORED_MODEL_SCALE: f32 = 1.0;

struct FoamRecord {
    center: [f32; 2],
    heading: f32,
    size0: f32,
    growth: f32,
    lifetime: f32,
    peak: f32,
    born: f32,
    ring: bool,
    patch: Patch,
}

struct Patch {
    triangles: Vec<Vec3>,
    surface: Entity,
}

struct Emitter {
    last_feet: Option<Vec3>,
    next_emission_at: f32,
    over_ring_line: bool,
    rng: u32,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            last_feet: None,
            next_emission_at: 0.0,
            over_ring_line: false,
            rng: 0x5EED_F0A5,
        }
    }
}

#[derive(Resource)]
struct WaterFoam {
    pool: Vec<Option<FoamRecord>>,
    cursor: usize,
    emitter: Option<Emitter>,
}

impl Default for WaterFoam {
    fn default() -> Self {
        Self {
            pool: (0..POOL_SIZE).map(|_| None).collect(),
            cursor: 0,
            emitter: None,
        }
    }
}

#[derive(Resource)]
struct FoamStencils {
    ring: Handle<Image>,
    wake: Handle<Image>,
}

/// A foam stencil's alpha is the shape, its dark colour the strength.
fn stencil(install: &Install, path: &str) -> Option<Image> {
    let blp = blp::decode(&install.0.read(path).ok()?).ok()?;
    let level = blp.mips.into_iter().next()?;
    let mut image = Image::new(
        Extent3d {
            width: level.width,
            height: level.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        level.rgba,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::default()
    });
    Some(image)
}

fn setup_foam(
    mut commands: Commands<'_, '_>,
    install: Res<'_, Install>,
    mut images: ResMut<'_, Assets<Image>>,
) {
    let (Some(ring), Some(wake)) = (
        stencil(&install, RING_TEXTURE),
        stencil(&install, WAKE_TEXTURE),
    ) else {
        warn!("no foam stencils: wading draws no foam");
        return;
    };
    commands.insert_resource(FoamStencils {
        ring: images.add(ring),
        wake: images.add(wake),
    });
}

fn build_patch(
    center: [f32; 2],
    final_size: f32,
    grids: &[(Entity, &LiquidGrid)],
) -> Option<Patch> {
    let (lo, hi) = (
        [center[0] - final_size, center[1] - final_size],
        [center[0] + final_size, center[1] + final_size],
    );
    let mut triangles = Vec::new();
    let mut surface = None;
    for &(entity, grid) in grids {
        if !grid.overlaps(lo, hi) {
            continue;
        }
        if surface.is_none() && grid.contains(center[0], center[1]) {
            surface = Some(entity);
        }
        grid.for_each_wet_cell(|CellCorners { tl, tr, bl, br }| {
            let xs = [tl[0], tr[0], bl[0], br[0]];
            let ys = [tl[1], tr[1], bl[1], br[1]];
            let (x0, x1) = (
                xs.into_iter().fold(f32::MAX, f32::min),
                xs.into_iter().fold(f32::MIN, f32::max),
            );
            let (y0, y1) = (
                ys.into_iter().fold(f32::MAX, f32::min),
                ys.into_iter().fold(f32::MIN, f32::max),
            );
            if x1 < lo[0] || x0 > hi[0] || y1 < lo[1] || y0 > hi[1] {
                return;
            }
            for v in [tl, bl, br, tl, br, tr] {
                triangles.push(wow_to_bevy(v));
            }
        });
    }
    let surface = surface?;
    (!triangles.is_empty()).then_some(Patch { triangles, surface })
}

fn wade_state(wader: &Viewer, vel: Vec3) -> WadeState {
    let w = bevy_to_wow(vel);
    if wader.translating {
        WadeState::Translating {
            speed: w[0].hypot(w[1]),
            heading: w[1].atan2(w[0]),
        }
    } else if wader.turning {
        WadeState::Turning
    } else {
        WadeState::Standing
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_foam(
    time: Res<'_, Time>,
    wader: Option<Res<'_, Viewer>>,
    stencils: Option<Res<'_, FoamStencils>>,
    index: Res<'_, WaterIndex>,
    grids: Query<'_, '_, &LiquidGrid, With<FoamPatch>>,
    mut foam: ResMut<'_, WaterFoam>,
) {
    if time.delta().is_zero() {
        return;
    }
    let (Some(wader), Some(_)) = (wader, stencils) else {
        return;
    };
    let feet = wader.body.filter(|_| wader.settled && !grids.is_empty());
    let Some(feet) = feet else {
        foam.emitter = None;
        return;
    };
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let WaterFoam {
        pool,
        cursor,
        emitter,
    } = &mut *foam;
    let emitter = emitter.get_or_insert_with(Emitter::default);
    let vel = emitter
        .last_feet
        .replace(feet)
        .map_or(Vec3::ZERO, |p| (feet - p) / dt);
    let state = wade_state(&wader, vel);
    let wow = bevy_to_wow(feet);
    let Some(surface) = index
        .0
        .over(wow[0], wow[1])
        .iter()
        .find_map(|&e| grids.get(e).ok()?.surface_z_at(wow[0], wow[1]))
    else {
        emitter.over_ring_line = false;
        return;
    };
    let h = wader.collision_height;
    let depth = surface - wow[2];
    let over_now = depth > ONESHOT_DEPTH_FRAC * h;
    let oneshot = over_now != emitter.over_ring_line;
    emitter.over_ring_line = over_now;
    if !oneshot && now < emitter.next_emission_at {
        return;
    }
    let gate = (GATE_DEPTH_FRAC * h).max(MIN_GATE_DEPTH);
    let Some(p) = foam_params(
        state,
        oneshot,
        AUTHORED_MODEL_SCALE,
        gate,
        depth,
        &mut emitter.rng,
    ) else {
        return;
    };
    let heading = match (p.ring, state) {
        (false, WadeState::Translating { heading, .. }) => heading,
        _ => rand01(&mut emitter.rng) * std::f32::consts::TAU,
    };
    let final_size = p.size0 + p.growth * p.lifetime;
    let center = [wow[0], wow[1]];
    let near: Vec<(Entity, &LiquidGrid)> = index
        .0
        .over_box(
            [center[0] - final_size, center[1] - final_size],
            [center[0] + final_size, center[1] + final_size],
        )
        .into_iter()
        .filter_map(|e| Some((e, grids.get(e).ok()?)))
        .collect();
    if let Some(patch) = build_patch(center, final_size, &near) {
        let slot = *cursor;
        *cursor = (slot + 1) % POOL_SIZE;
        pool[slot] = Some(FoamRecord {
            center,
            heading,
            size0: p.size0,
            growth: p.growth,
            lifetime: p.lifetime,
            peak: p.peak,
            born: now,
            ring: p.ring,
            patch,
        });
    }
    let mut uni = |a: f32, b: f32| a + (b - a) * rand01(&mut emitter.rng);
    emitter.next_emission_at = if oneshot {
        now
    } else if p.ring {
        now + uni(RING_INTERVAL_SECS.start, RING_INTERVAL_SECS.end)
    } else {
        let speed = match state {
            WadeState::Translating { speed, .. } => speed,
            _ => 0.0,
        };
        now + wake_cooldown(speed, &mut emitter.rng)
    };
}

fn push_foam(
    time: Res<'_, Time>,
    stencils: Option<Res<'_, FoamStencils>>,
    camera: Query<'_, '_, Entity, With<WorldCamera>>,
    surfaces: Query<'_, '_, (), With<LiquidGrid>>,
    mut foam: ResMut<'_, WaterFoam>,
    mut effects: ResMut<'_, EffectQuads>,
) {
    let (Some(stencils), Ok(cam)) = (stencils, camera.single()) else {
        return;
    };
    let now = time.elapsed_secs();
    for slot in &mut foam.pool {
        if slot.as_ref().is_some_and(|r| now - r.born >= r.lifetime) {
            *slot = None;
        }
    }
    let mut groups: BTreeMap<(Entity, bool), Vec<&FoamRecord>> = BTreeMap::new();
    for rec in foam.pool.iter().flatten() {
        if surfaces.contains(rec.patch.surface) {
            groups
                .entry((rec.patch.surface, rec.ring))
                .or_default()
                .push(rec);
        }
    }
    for ((_, ring), records) in groups {
        let start = effects.begin();
        let mut centroid = Vec3::ZERO;
        for rec in &records {
            let size = record_size(rec.size0, rec.growth, rec.born, now);
            let alpha = record_alpha(rec.peak, rec.lifetime, rec.born, now);
            for v in &rec.patch.triangles {
                let wow = bevy_to_wow(*v);
                effects.verts.push(EffectVertex {
                    pos: v.to_array(),
                    uv: foam_uv(rec.center, rec.heading, size, [wow[0], wow[1]]),
                    color: [1.0, 1.0, 1.0, alpha],
                });
                centroid += *v;
            }
        }
        let n = effects.verts.len() as u32 - start;
        if n == 0 {
            continue;
        }
        let stencil = if ring { &stencils.ring } else { &stencils.wake };
        effects.commit_tris(
            start,
            EffectDrawSpec {
                cam,
                texture: stencil.id(),
                blend: EffectBlend::Add,
                fog: EffectFog::Off,
                lighting: EffectLighting::None,
                sort_anchor: centroid / n as f32,
                sort_bias: FOAM_SORT_RUNG,
                raster_bias: FOAM_RASTER,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: Entity::PLACEHOLDER,
            },
        );
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WaterFoam>()
        .add_systems(Startup, setup_foam)
        .add_systems(Update, emit_foam.in_set(crate::WorldSystems))
        .add_systems(PostUpdate, push_foam.after(begin_effect_frame));
}

#[cfg(test)]
mod tests {
    use terrain::LiquidKind;

    use super::super::query::LiquidSource;
    use super::*;

    fn grid(wet: Vec<bool>) -> LiquidGrid {
        let positions = (0..3)
            .flat_map(|j| (0..3).map(move |i| [i as f32 * 5.0, j as f32 * 5.0, 5.0]))
            .collect();
        LiquidGrid::new(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            [3, 3],
            positions,
            wet,
        )
    }

    #[test]
    fn a_patch_is_the_wet_cells_it_overlaps_on_the_surface() {
        let g = grid(vec![true, false, false, false]);
        let near = [(Entity::PLACEHOLDER, &g)];
        let patch = build_patch([2.0, 2.0], 1.5, &near).expect("over water");
        assert_eq!(patch.triangles.len(), 6);
        assert_eq!(patch.surface, Entity::PLACEHOLDER);
        assert!(
            patch
                .triangles
                .iter()
                .all(|v| (bevy_to_wow(*v)[2] - 5.0).abs() < 1e-4)
        );
        assert!(build_patch([50.0, 50.0], 1.5, &near).is_none());
    }

    #[test]
    fn a_wader_steps_in_with_a_ring() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<WaterIndex>()
            .init_resource::<WaterFoam>()
            .insert_resource(Viewer {
                body: Some(wow_to_bevy([2.0, 2.0, 3.5])),
                settled: true,
                translating: false,
                turning: false,
                collision_height: 2.0,
            })
            .insert_resource(FoamStencils {
                ring: Handle::default(),
                wake: Handle::default(),
            })
            .add_systems(
                Update,
                (super::super::spatial::maintain_water_index, emit_foam).chain(),
            );
        app.world_mut().spawn((grid(vec![true; 4]), FoamPatch));
        let pooled = |app: &App| {
            app.world()
                .resource::<WaterFoam>()
                .pool
                .iter()
                .flatten()
                .count()
        };
        app.update();
        assert_eq!(pooled(&app), 0, "a held clock births nothing");
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_millis(16));
        app.update();
        let foam = app.world().resource::<WaterFoam>();
        let live: Vec<&FoamRecord> = foam.pool.iter().flatten().collect();
        assert_eq!(live.len(), 1);
        assert!(live[0].ring);
    }
}
