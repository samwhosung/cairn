use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use avian3d::prelude::{Collider, RigidBody};
use bevy::asset::AssetPlugin;
use bevy::ecs::world::CommandQueue;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use model::{
    ParticleEmitterDef, RibbonEmitterDef, parse_m2_animations, parse_m2_particle_emitters,
    parse_m2_playable_animation_lookup, parse_m2_ribbon_emitters,
};

use super::{EmitClock, EmitterFrames, MAX_PARTICLES, OwnerLoss, ParticleEmitter, spawn_emitter};
use crate::effects::EffectQuads;
use crate::m2::{M2Model, ModelEmitter, ModelRibbon};
use crate::model_material::{ModelMaterial, ModelMaterials};
use crate::portal::{CameraInteriorClaim, ExteriorWindows};
use crate::ribbons::{RibbonSeq, RibbonTrail, spawn_ribbon};

struct RecursionModel {
    name: String,
    emitters: Vec<ParticleEmitterDef>,
}

struct ModelRecords {
    emitters: Vec<ParticleEmitterDef>,
    ribbons: Vec<RibbonEmitterDef>,
    idle_seq: usize,
    recursion_models: Vec<Option<RecursionModel>>,
}

fn stand_seq(bytes: &[u8]) -> usize {
    let stand = parse_m2_playable_animation_lookup(bytes)
        .ok()
        .and_then(|l| l.first().map(|p| p.resolved_id))
        .unwrap_or(0);
    let anims = parse_m2_animations(bytes);
    anims
        .iter()
        .find(|a| a.anim_id == stand)
        .or_else(|| anims.first())
        .map_or(0, |a| a.seq_index)
}

fn m2_name(raw: &str) -> String {
    let p = raw.to_ascii_lowercase();
    let stem = p
        .strip_suffix(".mdx")
        .or_else(|| p.strip_suffix(".mdl"))
        .or_else(|| p.strip_suffix(".m2"))
        .unwrap_or(&p);
    format!("{stem}.m2")
}

fn records() -> Option<Vec<ModelRecords>> {
    let data = std::env::var_os("WOW_DATA").or_else(|| {
        eprintln!("skipped: WOW_DATA is not set");
        None
    })?;
    let chain = mpq::Chain::open(data).expect("open the chain");
    let carries =
        |b: &[u8]| b.len() >= 0x144 && [0x134, 0x13c].iter().any(|&at| b[at..at + 4] != [0; 4]);
    let mut out = Vec::new();
    for name in chain.list().into_iter().map(|e| e.name) {
        if !name.to_ascii_lowercase().ends_with(".m2") {
            continue;
        }
        let Ok(bytes) = chain.read(&name) else {
            continue;
        };
        if !carries(&bytes) {
            continue;
        }
        let emitters = parse_m2_particle_emitters(&bytes);
        let recursion_models = emitters
            .iter()
            .map(|d| {
                let name = m2_name(d.recursion_model.as_deref()?);
                let bytes = chain.read(&name).ok()?;
                let emitters = parse_m2_particle_emitters(&bytes);
                Some(RecursionModel { name, emitters })
            })
            .collect();
        out.push(ModelRecords {
            ribbons: parse_m2_ribbon_emitters(&bytes),
            idle_seq: stand_seq(&bytes),
            emitters,
            recursion_models,
        });
    }
    Some(out)
}

fn effects_model(emitters: Vec<ModelEmitter>) -> M2Model {
    M2Model {
        submeshes: Vec::new(),
        bounds: None,
        lights: Vec::new(),
        skeleton: crate::rig::ModelSkeleton::default(),
        inverse_bindposes: Arc::from([]),
        attachments: Vec::new(),
        animations: None,
        has_emitters: true,
        emitters,
        ribbons: Vec::new(),
    }
}

fn app(dt: Duration) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .add_plugins(crate::collision::physics_plugins(FixedPostUpdate))
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .init_asset::<M2Model>()
        .init_asset::<ModelMaterial>()
        .init_resource::<ModelMaterials>()
        .init_resource::<ExteriorWindows>()
        .init_resource::<CameraInteriorClaim>()
        .add_plugins((
            crate::effects::EffectsPlugin,
            super::ParticlePlugin,
            crate::ribbons::RibbonPlugin,
        ))
        .insert_resource(TimeUpdateStrategy::ManualDuration(dt));
    app.finish();
    app.cleanup();
    let eye = Transform::from_xyz(0.0, 3.0, 0.0).looking_to(Vec3::NEG_Z, Vec3::Y);
    let cam = app.world_mut().spawn(crate::view::world_camera(eye)).id();
    let frustum = {
        let world = app.world_mut();
        let mut projection = world.get_mut::<Projection>(cam).expect("a projection");
        if let Projection::Perspective(p) = &mut *projection {
            p.aspect_ratio = 16.0 / 9.0;
        }
        projection.compute_frustum(&GlobalTransform::from(eye))
    };
    app.world_mut()
        .entity_mut(cam)
        .insert((frustum, GlobalTransform::from(eye)));
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(400.0, 1.0, 400.0),
        Transform::from_xyz(0.0, -0.5, -100.0),
    ));
    app
}

fn seat(k: usize) -> Transform {
    Transform::from_xyz(
        (k % 40) as f32 * 2.0 - 40.0,
        0.0,
        -10.0 - (k / 40) as f32 * 2.0,
    )
    .with_rotation(Quat::from_rotation_y(
        (k * 37 % 360) as f32 * std::f32::consts::PI / 180.0,
    ))
}

fn carried(k: usize, t: f32) -> Transform {
    let a = t * 7.0 / 4.0 + k as f32;
    let mut at = seat(k);
    at.translation += Vec3::new(a.cos(), 0.0, a.sin()) * 4.0;
    at.rotation = Quat::from_rotation_y(-a);
    at
}

fn spawn_all(app: &mut App, models: &[ModelRecords]) -> Vec<Entity> {
    let world = app.world_mut();
    let texture = world.resource_mut::<Assets<Image>>().add(Image::default());
    let emitter = |def: &ParticleEmitterDef, idle: usize| ModelEmitter {
        def: def.clone(),
        texture: Some(texture.clone()),
        bone_pivot: [0.0; 3],
        recursion: None,
        geometry: None,
        owner_reach: 1.0,
        idle_seq_index: idle,
    };
    let mut recursion: HashMap<&str, Handle<M2Model>> = HashMap::new();
    for r in models
        .iter()
        .flat_map(|m| m.recursion_models.iter().flatten())
    {
        if !recursion.contains_key(r.name.as_str()) {
            let model = effects_model(r.emitters.iter().map(|d| emitter(d, 0)).collect());
            let handle = world.resource_mut::<Assets<M2Model>>().add(model);
            recursion.insert(&r.name, handle);
        }
    }
    let carriers: Vec<Entity> = (0..models.len())
        .map(|k| {
            world
                .spawn(if k % 2 == 1 { carried(k, 0.0) } else { seat(k) })
                .id()
        })
        .collect();
    let mut queue = CommandQueue::default();
    let mut commands = Commands::new(&mut queue, world);
    for (k, m) in models.iter().enumerate() {
        let moving = k % 2 == 1;
        for (def, model) in m.emitters.iter().zip(&m.recursion_models) {
            let em = ModelEmitter {
                recursion: model.as_ref().map(|r| recursion[r.name.as_str()].clone()),
                ..emitter(def, m.idle_seq)
            };
            let frames = EmitterFrames {
                owner: moving.then_some((carriers[k], [0.0; 3])),
                anchor: moving.then_some(carriers[k]),
                alpha: None,
                on_owner_loss: if k % 4 == 1 {
                    OwnerLoss::Free
                } else {
                    OwnerLoss::Drain
                },
            };
            spawn_emitter(&mut commands, &em, seat(k), frames, EmitClock::Pinned);
        }
        for def in &m.ribbons {
            let ribbon = ModelRibbon {
                def: def.clone(),
                texture: Some(texture.clone()),
                bone_pivot: [0.0; 3],
                owner_reach: 1.0,
            };
            let seq = RibbonSeq::Fixed(0);
            spawn_ribbon(
                &mut commands,
                &ribbon,
                carriers[k],
                false,
                1.0,
                seq,
                None,
                None,
            );
        }
    }
    queue.apply(world);
    carriers
}

fn finite(v: impl IntoIterator<Item = f32>) -> bool {
    v.into_iter().all(f32::is_finite)
}

struct FrameCounts {
    particles: usize,
    vertices: usize,
}

fn check(app: &mut App) -> FrameCounts {
    let mut particles = 0;
    let mut emitters = app.world_mut().query::<&ParticleEmitter>();
    for e in emitters.iter(app.world()) {
        let pools =
            std::iter::once(&e.particles).chain(e.recursion_emitters.iter().map(|c| &c.particles));
        for pool in pools {
            assert!(pool.len() <= MAX_PARTICLES, "a pool outgrew its cap");
            for p in pool {
                let q = p.quat;
                assert!(
                    finite(p.pos.to_array().into_iter().chain(p.vel.to_array()))
                        && finite([p.age, p.life, q.x, q.y, q.z, q.w])
                        && finite(p.angvel.to_array()),
                    "a particle went non-finite"
                );
            }
            particles += pool.len();
        }
    }
    let mut trails = app.world_mut().query::<&RibbonTrail>();
    for t in trails.iter(app.world()) {
        assert!(
            t.edge_count() <= crate::ribbons::MAX_EDGES,
            "a trail outgrew its cap"
        );
    }
    let quads = app.world().resource::<EffectQuads>();
    for v in &quads.verts {
        assert!(
            finite(v.pos.into_iter().chain(v.uv).chain(v.color)),
            "a vertex went non-finite"
        );
    }
    FrameCounts {
        particles,
        vertices: quads.verts.len(),
    }
}

#[test]
fn every_emitter_of_the_install_runs_at_30_and_144_hz() {
    let Some(models) = records() else {
        return;
    };
    let emitters: usize = models.iter().map(|m| m.emitters.len()).sum();
    let ribbons: usize = models.iter().map(|m| m.ribbons.len()).sum();
    for (hz, seconds) in [(30u32, 3), (144, 2)] {
        let started = Instant::now();
        let dt = Duration::from_nanos(1_000_000_000 / u64::from(hz));
        let mut app = app(dt);
        let carriers = spawn_all(&mut app, &models);
        let (frames, gone) = (seconds * hz as usize, (seconds * hz as usize) * 2 / 3);
        let (mut most, mut verts) = (0, 0);
        for f in 0..frames {
            let t = (f + 1) as f32 * dt.as_secs_f32();
            for (k, &c) in carriers.iter().enumerate().filter(|(k, _)| k % 2 == 1) {
                if f == gone {
                    app.world_mut().despawn(c);
                } else if f < gone {
                    *app.world_mut().get_mut::<Transform>(c).expect("a carrier") = carried(k, t);
                }
            }
            app.update();
            let counts = check(&mut app);
            most = most.max(counts.particles);
            verts += counts.vertices;
        }
        eprintln!(
            "{hz} Hz: {} models, {emitters} emitters, {ribbons} ribbons, {frames} frames: at most \
             {most} particles live, {verts} vertices drawn [{:.1?}]",
            models.len(),
            started.elapsed()
        );
        assert!(most > 0 && verts > 0, "nothing ran");
    }
}
