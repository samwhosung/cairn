//! The foam a body makes wading: a wake behind it while it moves, rings while it stands or turns,
//! and one ring as it steps in or out. Each emission is a record in a small pool, its patch cut
//! once from the wet cells under its final size; it grows by stretching its texture over that
//! patch and fades in and out over its short life.

mod params;

use bevy::asset::RenderAssetUsages;
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::NoAutoAabb;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::drift::rand01;
use super::query::LiquidGrid;
use super::{FoamPatch, WaterIndex};
use crate::Install;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::effect::{EffectLook, EffectMaterial, effect_material};
use crate::interior::Viewer;
use crate::light::LightBuffer;
use params::{
    RING_INTERVAL, WadeState, foam_params, foam_uv, record_alpha, record_size, wake_cooldown,
};

const RING_TEXTURE: &str = "XTextures\\splash\\splash.blp";
const WAKE_TEXTURE: &str = "XTextures\\splash\\wake.blp";
/// The avatar's share of the client's foam pool; the oldest record is overwritten.
const POOL_SIZE: usize = 32;
/// The one-off ring fires as the water over the feet crosses this fraction of the body's height.
const ONESHOT_DEPTH_FRAC: f32 = 0.4;
/// A body foams in water no deeper than this many of its heights, and never under a yard.
const GATE_DEPTH_FRAC: f32 = 2.0;
/// After every liquid surface, before the rest of the transparent pass.
const FOAM_SORT: f32 = -1.0e4;
/// A few steps of depth toward the eye, against a driver rounding the two coplanar draws apart.
const FOAM_RASTER: i32 = 8;

/// The avatar's model scale; cairn's bodies are drawn at their authored size.
const WADER_SCALE: f32 = 1.0;

struct FoamRecord {
    center: [f32; 2],
    heading: f32,
    size0: f32,
    growth: f32,
    lifetime: f32,
    peak: f32,
    born: f32,
    ring: bool,
    /// The patch's triangles, Bevy space.
    verts: Vec<Vec3>,
    /// The surface it was emitted over; the record is not drawn once that streams out.
    host: Entity,
}

/// The wader's emitter: it starts afresh whenever the body or every surface goes away.
struct Emitter {
    last_feet: Option<Vec3>,
    /// When the next emission may come.
    ready: f32,
    /// Deeper than the one-off ring's line.
    wading: bool,
    rng: u32,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            last_feet: None,
            ready: 0.0,
            wading: false,
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

/// The ring and wake draws: one mesh each, rebuilt every frame from the live records.
#[derive(Resource)]
struct FoamDraws {
    ring: (Entity, Handle<Mesh>),
    wake: (Entity, Handle<Mesh>),
}

/// The stencil's first level as stored, clamped: its alpha is the shape, its dark colour the
/// strength.
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
    light: Option<Res<'_, LightBuffer>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut materials: ResMut<'_, Assets<EffectMaterial>>,
) {
    let Some(light) = light else {
        return;
    };
    let (Some(ring), Some(wake)) = (
        stencil(&install, RING_TEXTURE),
        stencil(&install, WAKE_TEXTURE),
    ) else {
        warn!("no foam stencils: wading draws no foam");
        return;
    };
    let look = EffectLook {
        additive: true,
        fogged: false,
        camera_relative: false,
        sort: FOAM_SORT,
        raster_bias: FOAM_RASTER,
    };
    let mut draw = |image: Image| {
        let material = materials.add(effect_material(look, images.add(image), &light.0));
        let mesh = meshes.add(patch_mesh(Vec::new(), Vec::new(), Vec::new()));
        let entity = commands
            .spawn((
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                Transform::IDENTITY,
                Visibility::Hidden,
                Aabb::default(),
                NoAutoAabb,
            ))
            .id();
        (entity, mesh)
    };
    let draws = FoamDraws {
        ring: draw(ring),
        wake: draw(wake),
    };
    commands.insert_resource(draws);
}

/// Every wet cell of the surfaces near a record's final box that overlaps it, as the liquid's
/// own two triangles, on the surface, and the first surface under the centre; `None` when the
/// centre is over no surface.
fn build_patch(
    center: [f32; 2],
    final_size: f32,
    grids: &[(Entity, &LiquidGrid)],
) -> Option<(Vec<Vec3>, Entity)> {
    let (lo, hi) = (
        [center[0] - final_size, center[1] - final_size],
        [center[0] + final_size, center[1] + final_size],
    );
    let mut verts = Vec::new();
    let mut host = None;
    for &(entity, grid) in grids {
        if !grid.overlaps(lo, hi) {
            continue;
        }
        if host.is_none() && grid.contains(center[0], center[1]) {
            host = Some(entity);
        }
        grid.for_each_wet_cell(|[tl, tr, bl, br]| {
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
                verts.push(wow_to_bevy(v));
            }
        });
    }
    let host = host?;
    (!verts.is_empty()).then_some((verts, host))
}

#[allow(clippy::too_many_arguments)]
fn emit_foam(
    time: Res<'_, Time>,
    wader: Option<Res<'_, Viewer>>,
    draws: Option<Res<'_, FoamDraws>>,
    index: Res<'_, WaterIndex>,
    grids: Query<'_, '_, &LiquidGrid, With<FoamPatch>>,
    mut foam: ResMut<'_, WaterFoam>,
) {
    let (Some(wader), Some(_)) = (wader, draws) else {
        return;
    };
    let feet = wader.body.filter(|_| wader.settled && !grids.is_empty());
    let Some(feet) = feet else {
        foam.emitter = None;
        return;
    };
    let now = time.elapsed_secs();
    let dt = time.delta_secs().max(1.0e-4);
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
    let w = bevy_to_wow(vel);
    let state = if wader.translating {
        WadeState::Translating {
            speed: w[0].hypot(w[1]),
            heading: w[1].atan2(w[0]),
        }
    } else if wader.turning {
        WadeState::Turning
    } else {
        WadeState::Standing
    };
    let wow = bevy_to_wow(feet);
    let Some(surface) = index
        .0
        .over(wow[0], wow[1])
        .iter()
        .find_map(|&e| grids.get(e).ok()?.surface_z_at(wow[0], wow[1]))
    else {
        emitter.wading = false;
        return;
    };
    let h = wader.height;
    let depth = surface - wow[2];
    let wading_now = depth > ONESHOT_DEPTH_FRAC * h;
    let oneshot = wading_now != emitter.wading;
    emitter.wading = wading_now;
    if !oneshot && now < emitter.ready {
        return;
    }
    let gate = (GATE_DEPTH_FRAC * h).max(1.0);
    let Some(p) = foam_params(state, oneshot, WADER_SCALE, gate, depth, &mut emitter.rng) else {
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
    if let Some((verts, host)) = build_patch(center, final_size, &near) {
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
            verts,
            host,
        });
    }
    let mut uni = |a: f32, b: f32| a + (b - a) * rand01(&mut emitter.rng);
    emitter.ready = if oneshot {
        now
    } else if p.ring {
        now + uni(RING_INTERVAL.0, RING_INTERVAL.1)
    } else {
        let speed = match state {
            WadeState::Translating { speed, .. } => speed,
            _ => 0.0,
        };
        now + wake_cooldown(speed, &mut emitter.rng)
    };
}

fn patch_mesh(positions: Vec<[f32; 3]>, uvs: Vec<[f32; 2]>, colors: Vec<[f32; 4]>) -> Mesh {
    let n = positions.len() as u32;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32((0..n).collect()));
    mesh
}

/// Live records drawn white at their alpha, their texture stretched to their size now. Each
/// draw sorts at the centre of its vertices.
fn draw_foam(
    time: Res<'_, Time>,
    draws: Option<Res<'_, FoamDraws>>,
    hosts: Query<'_, '_, (), With<LiquidGrid>>,
    mut foam: ResMut<'_, WaterFoam>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut placed: Query<'_, '_, (&mut Visibility, &mut Aabb)>,
) {
    let Some(draws) = draws else {
        return;
    };
    let now = time.elapsed_secs();
    for slot in &mut foam.pool {
        if slot.as_ref().is_some_and(|r| now - r.born >= r.lifetime) {
            *slot = None;
        }
    }
    for (ring, (entity, mesh)) in [(true, &draws.ring), (false, &draws.wake)] {
        let (mut positions, mut uvs, mut colors) = (Vec::new(), Vec::new(), Vec::new());
        let live = foam.pool.iter().flatten();
        for rec in live.filter(|r| r.ring == ring && hosts.contains(r.host)) {
            let size = record_size(rec.size0, rec.growth, rec.born, now);
            let alpha = record_alpha(rec.peak, rec.lifetime, rec.born, now);
            for v in &rec.verts {
                let wow = bevy_to_wow(*v);
                positions.push(v.to_array());
                uvs.push(foam_uv(rec.center, rec.heading, size, [wow[0], wow[1]]));
                colors.push([1.0, 1.0, 1.0, alpha]);
            }
        }
        let Ok((mut vis, mut aabb)) = placed.get_mut(*entity) else {
            continue;
        };
        if positions.is_empty() {
            vis.set_if_neq(Visibility::Hidden);
            continue;
        }
        let centre =
            positions.iter().map(|&p| Vec3::from(p)).sum::<Vec3>() / positions.len() as f32;
        let reach = positions
            .iter()
            .fold(Vec3::ZERO, |r, &p| r.max((Vec3::from(p) - centre).abs()));
        *aabb = Aabb {
            center: centre.into(),
            half_extents: reach.into(),
        };
        if let Some(m) = meshes.get_mut(mesh) {
            *m = patch_mesh(positions, uvs, colors);
        }
        vis.set_if_neq(Visibility::Inherited);
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<WaterFoam>()
        .add_systems(Startup, setup_foam)
        .add_systems(Update, emit_foam.in_set(crate::WorldSystems))
        .add_systems(
            PostUpdate,
            draw_foam.before(bevy::transform::TransformSystems::Propagate),
        );
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
        let (verts, host) = build_patch([2.0, 2.0], 1.5, &near).expect("over water");
        assert_eq!((verts.len(), host), (6, Entity::PLACEHOLDER));
        assert!(
            verts
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
                height: 2.0,
            })
            .insert_resource(FoamDraws {
                ring: (Entity::PLACEHOLDER, Handle::default()),
                wake: (Entity::PLACEHOLDER, Handle::default()),
            })
            .add_systems(
                Update,
                (super::super::spatial::maintain_water_index, emit_foam).chain(),
            );
        app.world_mut().spawn((grid(vec![true; 4]), FoamPatch));
        app.update();
        let foam = app.world().resource::<WaterFoam>();
        let live: Vec<&FoamRecord> = foam.pool.iter().flatten().collect();
        assert_eq!(live.len(), 1);
        assert!(live[0].ring);
    }
}
