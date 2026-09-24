use avian3d::prelude::SpatialQuery;
use bevy::camera::Projection;
use bevy::camera::primitives::Frustum;
use bevy::prelude::*;
use model::{ParamsNow, ParticleEmitterDef};

use super::quads::{CamBasis, DrawFrame, expand_quads};
use super::{
    DrawSetGate, GeometryParticleMesh, MAX_PARTICLES, MAX_STEP, OwnerLoss, Particle,
    ParticleEmitter, RecursionEmitter, SceneGates, accumulate_emission, emit::emit_local,
    emit::emitter_frame_turn_rotation, rand_signed, rand01, xorshift32,
};
use crate::collision::WorldCollision;
use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::effects::{EffectDrawSpec, EffectFog, EffectLighting, EffectQuads};
use crate::rig::ModelAnimations;
use crate::unit::UnitAlpha;
use crate::view::{WorldCamera, nearest_depth_within_farclip};

struct StepEnv {
    dt: f32,
    gravity: f32,
    drag: f32,
    world_space: bool,
    kill_origin: Option<Vec3>,
    follow_delta: Vec3,
}

fn survives_step(p: &mut Particle, env: &StepEnv) -> bool {
    let (dt, g) = (env.dt, env.gravity);
    p.age += dt;
    if p.age >= p.life {
        return false;
    }
    if p.fresh {
        p.fresh = false;
    } else {
        p.pos += env.follow_delta;
    }
    let theta = p.angvel.length();
    if theta > 1e-4 {
        p.quat = (p.quat * Quat::from_axis_angle(p.angvel / theta, theta * dt)).normalize();
    }
    let step_vel = p.vel;
    p.pos += p.vel * dt;
    if env.world_space {
        p.pos.y -= 0.5 * g * dt * dt;
        p.vel.y -= g * dt;
    } else {
        p.pos.z -= 0.5 * g * dt * dt;
        p.vel.z -= g * dt;
    }
    if env.drag != 0.0 {
        let f = (dt * env.drag).min(1.0);
        p.vel -= f * p.vel;
    }
    if let Some(origin) = env.kill_origin
        && step_vel.dot(p.pos - origin) > 0.0
    {
        return false;
    }
    true
}

const FULL_EMISSION_YARDS: f32 = 50.0;
const EMISSION_FALLOFF_PER_YARD: f32 = 0.02;
const MIN_EMISSION_SCALE: f32 = 0.25;

fn emission_scale(distance: f32) -> f32 {
    (1.0 - (distance - FULL_EMISSION_YARDS) * EMISSION_FALLOFF_PER_YARD)
        .clamp(MIN_EMISSION_SCALE, 1.0)
}

fn follow_fraction(def: &ParticleEmitterDef, speed: f32) -> f32 {
    if !def.follow_emitter() {
        return 0.0;
    }
    def.follow_line().map_or(0.0, |(slope, intercept)| {
        (slope * speed + intercept).clamp(0.0, 1.0)
    })
}

const INHERIT_RETAKE_INTERVAL: f32 = 1.0 / 30.0;

fn retake_inherited_velocity(
    accum: &mut f32,
    held: &mut Vec3,
    dt: f32,
    delta: Vec3,
    live: bool,
    scale: f32,
) {
    *accum += dt;
    if *accum > INHERIT_RETAKE_INTERVAL {
        *held = if live {
            delta * (INHERIT_RETAKE_INTERVAL / *accum) * scale
        } else {
            Vec3::ZERO
        };
        *accum = 0.0;
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_at_parent_particles(
    child: &mut RecursionEmitter,
    now: &ParamsNow,
    parent: &[Particle],
    rate: f32,
    emitting: bool,
    scale: f32,
    dt: f32,
    world_space: bool,
    placement: &Transform,
) {
    let origin = Vec3::from(child.def.position);
    for p in parent {
        accumulate_emission(
            child.def.burst(),
            rate,
            emitting,
            scale,
            dt,
            &mut child.accumulator,
            &mut child.gate_prev,
        );
        while child.accumulator >= 1.0 && child.particles.len() < MAX_PARTICLES {
            child.accumulator -= 1.0;
            let (base, dir) = emit_local(&child.def, now, &mut child.rng);
            let local = base - origin;
            let speed =
                now.emission_speed * (1.0 + now.speed_variation * rand_signed(&mut child.rng));
            let fold = |v: Vec3| {
                if world_space {
                    placement.rotation * (placement.scale * wow_to_bevy(v.to_array()))
                } else {
                    v
                }
            };
            let mut vel = fold(dir * speed);
            if child.def.inherits_emitter_motion() {
                vel += (1.0 + now.speed_variation * rand_signed(&mut child.rng)) * p.vel;
            }
            let phase = xorshift32(&mut child.rng);
            child.particles.push(Particle {
                pos: p.pos + fold(local),
                vel,
                age: birth_age(child.def.burst(), dt, &mut child.rng),
                life: now.lifespan,
                phase,
                fresh: true,
                quat: Quat::IDENTITY,
                angvel: Vec3::ZERO,
            });
        }
    }
}

fn birth_age(is_burst: bool, dt: f32, rng: &mut u32) -> f32 {
    if is_burst {
        return 0.0;
    }
    rand01(rng) * dt.min(MAX_STEP)
}

pub(crate) struct PlayingSeq {
    pub(crate) slot: usize,
    pub(crate) seconds: f32,
}

pub(crate) fn playing_seq(player: &AnimationPlayer, anims: &ModelAnimations) -> Option<PlayingSeq> {
    player
        .playing_animations()
        .filter_map(|(node, active)| {
            let clip = anims.clips.iter().find(|c| c.node == *node)?;
            Some((clip.seq_index, active.seek_time(), active.weight()))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(slot, seconds, _)| PlayingSeq { slot, seconds })
        .or_else(|| {
            let idle = anims.idle_clip()?;
            Some(PlayingSeq {
                slot: idle.seq_index,
                seconds: 0.0,
            })
        })
}

type OwnerTransforms<'w, 's> = Query<
    'w,
    's,
    &'static GlobalTransform,
    (Without<ParticleEmitter>, Without<GeometryParticleMesh>),
>;

type Emitters<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut ParticleEmitter,
        &'static mut Transform,
        &'static mut GlobalTransform,
        Option<Ref<'static, DrawSetGate>>,
    ),
    Without<WorldCamera>,
>;

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn simulate_particles(
    time: Res<'_, Time>,
    mut commands: Commands<'_, '_>,
    cam: Query<'_, '_, (Entity, &GlobalTransform, &Frustum, &Projection), With<WorldCamera>>,
    owners: OwnerTransforms<'_, '_>,
    hidden: Query<'_, '_, &InheritedVisibility>,
    mut child_draws: Query<
        '_,
        '_,
        &mut Visibility,
        (With<GeometryParticleMesh>, Without<WorldCamera>),
    >,
    images: Res<'_, Assets<Image>>,
    mut quads: ResMut<'_, EffectQuads>,
    spatial: SpatialQuery<'_, '_>,
    alphas: Query<'_, '_, &UnitAlpha>,
    gates: SceneGates<'_, '_>,
    hosts: Query<'_, '_, (&AnimationPlayer, &ModelAnimations)>,
    mut emitters: Emitters<'_, '_>,
) {
    let Ok((world_cam, cam_tf, frustum, projection)) = cam.single() else {
        return;
    };
    let view = gates.view(cam_tf, projection, frustum);
    let dt = time.delta_secs().min(MAX_STEP);
    let stepping = dt > 0.0;
    let cam_pos = cam_tf.translation();
    let snap_filter = WorldCollision::body_filter();
    let (_, cam_rot, _) = cam_tf.to_scale_rotation_translation();
    let cam_basis = CamBasis {
        right: cam_rot * Vec3::X,
        up: cam_rot * Vec3::Y,
    };
    let cam_fwd = Vec3::from(cam_tf.forward());
    let mut hide_instances = |emitter: &mut ParticleEmitter| {
        if emitter.gated {
            return;
        }
        emitter.gated = true;
        for slot in &emitter.model_instances {
            for (e, _) in &slot.tinted_meshes {
                if let Ok(mut v) = child_draws.get_mut(*e) {
                    *v = Visibility::Hidden;
                }
            }
        }
    };
    for (entity, mut emitter, mut entity_tf, mut entity_global, fade) in &mut emitters {
        if let Some(f) = fade.as_ref() {
            if !f.admitted(&gates, &view) {
                hide_instances(&mut emitter);
                continue;
            }
            emitter.gated = false;
        } else if !emitter.draining
            && let Some(at) = owner_position(&emitter, &owners)
        {
            let owner_hidden = emitter
                .owner
                .is_some_and(|o| matches!(hidden.get(o), Ok(v) if !v.get()));
            if owner_hidden || !nearest_depth_within_farclip(cam_pos, cam_fwd, at, 0.0) {
                hide_instances(&mut emitter);
                continue;
            }
            emitter.gated = false;
        }
        if stepping {
            let drawable = step_emitter(
                entity,
                &mut emitter,
                fade.as_deref(),
                dt,
                cam_pos,
                &owners,
                &hosts,
                &alphas,
                &spatial,
                &snap_filter,
                &mut commands,
            );
            if !drawable {
                continue;
            }
        } else if emitter.owner.is_some_and(|o| owners.get(o).is_err()) {
            continue;
        }
        draw_emitter(
            entity,
            &emitter,
            &mut entity_tf,
            &mut entity_global,
            world_cam,
            &cam_basis,
            &images,
            &mut quads,
        );
    }
}

fn owner_position(emitter: &ParticleEmitter, owners: &OwnerTransforms<'_, '_>) -> Option<Vec3> {
    match emitter.owner {
        Some(o) => owners.get(o).ok().map(GlobalTransform::translation),
        None => Some(emitter.anchor_pos),
    }
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn step_emitter(
    entity: Entity,
    emitter: &mut ParticleEmitter,
    fade: Option<&DrawSetGate>,
    dt: f32,
    cam_pos: Vec3,
    owners: &OwnerTransforms<'_, '_>,
    hosts: &Query<'_, '_, (&AnimationPlayer, &ModelAnimations)>,
    alphas: &Query<'_, '_, &UnitAlpha>,
    spatial: &SpatialQuery<'_, '_>,
    snap_filter: &avian3d::prelude::SpatialQueryFilter,
    commands: &mut Commands<'_, '_>,
) -> bool {
    let ParticleEmitter {
        def,
        placement,
        owner,
        on_owner_loss,
        draining,
        alpha_src,
        alpha,
        anchor,
        anchor_pos,
        particles,
        accumulator,
        emitter_prev,
        inherit_accum,
        inherit_vel,
        gate_prev,
        age,
        host,
        seq,
        rng,
        recursion_emitters,
        model_instances,
        ..
    } = emitter;
    *age += dt;
    let (clock_seq, elapsed_s) = match *host {
        Some(h) => match hosts.get(h).ok().and_then(|(p, a)| playing_seq(p, a)) {
            Some(playing) => {
                *seq = Some(playing.slot);
                (Some(playing.slot), playing.seconds)
            }
            None => (*seq, 0.0),
        },
        None => (*seq, *age),
    };
    let gseq_now = f64::from(*age);
    let world_space = !def.model_space();
    if let Some(o) = *owner {
        if let Ok(gt) = owners.get(o) {
            *placement = gt.compute_transform();
        } else {
            *owner = None;
            match *on_owner_loss {
                OwnerLoss::Free => {
                    for slot in model_instances.iter() {
                        for (e, _) in &slot.tinted_meshes {
                            commands.entity(*e).despawn();
                        }
                    }
                    commands.entity(entity).despawn();
                    return false;
                }
                OwnerLoss::Drain => *draining = true,
            }
        }
    }
    let all_empty =
        particles.is_empty() && recursion_emitters.iter().all(|c| c.particles.is_empty());
    if *draining && all_empty {
        for slot in model_instances.iter() {
            for (e, _) in &slot.tinted_meshes {
                commands.entity(*e).despawn();
            }
        }
        commands.entity(entity).despawn();
        return false;
    }
    if all_empty && !*draining && !def.timing.emitting(clock_seq, elapsed_s, gseq_now) {
        return false;
    }
    *alpha = alpha_src.map_or(1.0, |e| alphas.get(e).map_or(1.0, |a| a.alpha))
        * fade.map_or(1.0, |f| f.distance_alpha(cam_pos));
    match *anchor {
        Some(a) => {
            if let Ok(gt) = owners.get(a) {
                *anchor_pos = gt.translation();
            }
        }
        None if owner.is_none() => *anchor_pos = placement.translation,
        None => {}
    }
    let emit_place = *placement;
    let emitter_world = emit_place.transform_point(wow_to_bevy(def.position));
    let emitter_delta = emitter_prev.map_or(Vec3::ZERO, |prev| emitter_world - prev);
    *emitter_prev = Some(emitter_world);
    let to_stored = |world: Vec3, placement: &Transform| {
        if world_space {
            world
        } else {
            Vec3::from(bevy_to_wow(
                (placement.rotation.inverse() * world) / placement.scale.max(Vec3::splat(1e-6)),
            ))
        }
    };
    let fraction = follow_fraction(def, emitter_delta.length() / dt);
    let follow = if fraction == 0.0 || emitter_delta == Vec3::ZERO {
        Vec3::ZERO
    } else {
        to_stored(fraction * emitter_delta, &emit_place)
    };
    if def.inherits_emitter_motion() {
        retake_inherited_velocity(
            inherit_accum,
            inherit_vel,
            dt,
            emitter_delta,
            !particles.is_empty(),
            def.inherit_scale,
        );
    }
    let now = def.params.sample(clock_seq, elapsed_s, gseq_now);
    let kill_origin = def.kill_outbound().then(|| {
        if world_space {
            emitter_world
        } else {
            Vec3::from(def.position)
        }
    });
    let env = StepEnv {
        dt,
        gravity: now.gravity,
        drag: def.drag,
        world_space,
        kill_origin,
        follow_delta: follow,
    };
    particles.retain_mut(|p| survives_step(p, &env));
    let emitting = !*draining && def.timing.emitting(clock_seq, elapsed_s, gseq_now);
    let rate = def.timing.rate(clock_seq, elapsed_s, gseq_now);
    let emission_scale = emission_scale(placement.translation.distance(cam_pos));
    accumulate_emission(
        def.burst(),
        rate,
        emitting,
        emission_scale,
        dt,
        accumulator,
        gate_prev,
    );
    while *accumulator >= 1.0 && particles.len() < MAX_PARTICLES {
        *accumulator -= 1.0;
        let (base, dir) = emit_local(def, &now, rng);
        let speed = now.emission_speed * (1.0 + now.speed_variation * (rand01(rng) * 2.0 - 1.0));
        let (mut pos, vel) = if world_space {
            (
                emit_place.transform_point(wow_to_bevy(base.to_array())),
                emit_place.rotation * (emit_place.scale * wow_to_bevy((dir * speed).to_array())),
            )
        } else {
            (base, dir * speed)
        };
        if world_space
            && def.ground_snap()
            && let Some(hit) = spatial.cast_ray(pos, Dir3::NEG_Y, 20.0, true, snap_filter)
        {
            pos = pos.with_y(pos.y - hit.distance + def.over_life.sample(0.0).size);
        }
        let vel = if def.inherits_emitter_motion() && *inherit_vel != Vec3::ZERO {
            vel + (1.0 + now.speed_variation * rand_signed(rng))
                * to_stored(*inherit_vel, &emit_place)
        } else {
            vel
        };
        let (quat, angvel) = if def.geometry_model.is_some() {
            geometry_spin(def, world_space, &emit_place, rng)
        } else {
            (Quat::IDENTITY, Vec3::ZERO)
        };
        let phase = xorshift32(rng);
        particles.push(Particle {
            pos,
            vel,
            age: birth_age(def.burst(), dt, rng),
            life: now.lifespan,
            phase,
            fresh: true,
            quat,
            angvel,
        });
    }
    for child in recursion_emitters.iter_mut() {
        let child_now = f64::from(*age);
        let c_emitting = !*draining && child.def.timing.emitting(None, *age, child_now);
        let c_rate = child.def.timing.rate(None, *age, child_now);
        let c_params = child.def.params.sample(None, *age, child_now);
        emit_at_parent_particles(
            child,
            &c_params,
            particles,
            c_rate,
            c_emitting,
            emission_scale,
            dt,
            world_space,
            &emit_place,
        );
        let c_env = StepEnv {
            dt,
            gravity: c_params.gravity,
            drag: child.def.drag,
            world_space,
            kill_origin: None,
            follow_delta: Vec3::ZERO,
        };
        child.particles.retain_mut(|p| survives_step(p, &c_env));
    }
    true
}

fn geometry_spin(
    def: &ParticleEmitterDef,
    world_space: bool,
    emit_place: &Transform,
    rng: &mut u32,
) -> (Quat, Vec3) {
    let amin = def.angular_velocity_min;
    let amax = def.angular_velocity_max;
    let mut w = [
        amin[0] + rand01(rng) * (amax[0] - amin[0]),
        (1.0 + rand01(rng)) * (amax[1] - amin[1]),
        (1.0 + rand01(rng)) * (amax[2] - amin[2]),
    ];
    if def.tumble_random_sign() {
        for a in &mut w {
            if xorshift32(rng) & 1 == 0 {
                *a = -*a;
            }
        }
    }
    let turn = emitter_frame_turn_rotation();
    let quat = if world_space {
        emit_place.rotation * turn
    } else {
        turn
    };
    (quat, wow_to_bevy(w))
}

#[allow(clippy::too_many_arguments)]
fn draw_emitter(
    entity: Entity,
    emitter: &ParticleEmitter,
    entity_tf: &mut Transform,
    entity_global: &mut GlobalTransform,
    cam: Entity,
    cam_basis: &CamBasis,
    images: &Assets<Image>,
    quads: &mut EffectQuads,
) {
    let def = &emitter.def;
    let world_space = !def.model_space();
    let anchor = if world_space {
        emitter.anchor_pos
    } else {
        emitter.placement.translation
    };
    entity_tf.translation = anchor;
    *entity_global = GlobalTransform::from(*entity_tf);
    let frame = DrawFrame {
        world_space,
        alpha: emitter.alpha,
    };
    let bias = model::owner_last_rung(emitter.owner_reach);
    let pools = std::iter::once((def, &emitter.particles, &emitter.texture)).chain(
        emitter
            .recursion_emitters
            .iter()
            .map(|c| (&c.def, &c.particles, &c.texture)),
    );
    for (i, (def, particles, texture)) in pools.enumerate() {
        let parent = i == 0;
        if parent && def.geometry_model.is_some() {
            continue;
        }
        if particles.is_empty() || !images.contains(texture) {
            continue;
        }
        let start = quads.begin();
        expand_quads(
            def,
            particles,
            &frame,
            &emitter.placement,
            cam_basis,
            &mut quads.verts,
        );
        quads.commit_quads(
            start,
            EffectDrawSpec {
                cam,
                texture: texture.id(),
                blend: def.blend.into(),
                fog: EffectFog::for_blend(def.flags, def.blend),
                lighting: if def.lit {
                    EffectLighting::Scene
                } else {
                    EffectLighting::None
                },
                sort_anchor: anchor,
                sort_bias: bias,
                raster_bias: 0,
                raster_slope: 0.0,
                cam_relative: false,
                no_depth_test: false,
                main_entity: entity,
            },
        );
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn particle(pos: Vec3, vel: Vec3) -> Particle {
        Particle {
            pos,
            vel,
            age: 0.0,
            life: 10.0,
            phase: 0,
            fresh: false,
            quat: Quat::IDENTITY,
            angvel: Vec3::ZERO,
        }
    }

    fn env(kill_origin: Option<Vec3>, follow: Vec3) -> StepEnv {
        StepEnv {
            dt: 0.1,
            gravity: 0.0,
            drag: 0.0,
            world_space: true,
            kill_origin,
            follow_delta: follow,
        }
    }

    #[test]
    fn a_burst_is_born_at_age_zero_and_a_pour_within_its_frame() {
        let mut rng = 0x1234_5678;
        assert!((0..64).all(|_| birth_age(true, 0.016, &mut rng) == 0.0));
        let mut rng = 0x9e37_79b9;
        assert!((0..256).all(|_| (0.0..0.016).contains(&birth_age(false, 0.016, &mut rng))));
        assert!((0..256).all(|_| (0.0..0.1).contains(&birth_age(false, 5.0, &mut rng))));
    }

    #[test]
    fn an_outbound_particle_dies_crossing_the_centre() {
        let origin = Vec3::new(1.0, 2.0, 3.0);
        let mut p = particle(origin + Vec3::X * 0.5, -Vec3::X * 2.0);
        let kill = env(Some(origin), Vec3::ZERO);
        assert!(survives_step(&mut p, &kill));
        assert!(survives_step(&mut p, &kill));
        assert!(!survives_step(&mut p, &kill));
        let mut free = particle(origin + Vec3::X * 0.5, -Vec3::X * 2.0);
        assert!((0..5).all(|_| survives_step(&mut free, &env(None, Vec3::ZERO))));
    }

    #[test]
    fn the_follow_motion_skips_a_particles_first_step() {
        let following = env(None, Vec3::X * 0.5);
        let mut p = particle(Vec3::ZERO, Vec3::ZERO);
        p.fresh = true;
        assert!(survives_step(&mut p, &following));
        assert_eq!(p.pos, Vec3::ZERO);
        assert!(survives_step(&mut p, &following));
        assert_eq!(p.pos, Vec3::X * 0.5);
    }

    #[test]
    fn the_follow_fraction_is_the_authored_line_clamped() {
        let plain = super::super::emit::tests::def(model::ParticleShape::Plane);
        assert_eq!(follow_fraction(&plain, 30.0), 0.0);
        let following = ParticleEmitterDef {
            flags: 0x4000,
            follow_speed1: 2.5,
            follow_scale1: 0.1,
            follow_speed2: 16.667,
            follow_scale2: 0.9,
            ..plain.clone()
        };
        assert!((follow_fraction(&following, 2.5) - 0.1).abs() < 1e-3);
        assert_eq!(follow_fraction(&following, 40.0), 1.0);
        let degenerate = ParticleEmitterDef {
            flags: 0x4000,
            follow_speed1: 4.0,
            follow_speed2: 4.0,
            ..plain
        };
        assert_eq!(follow_fraction(&degenerate, 30.0), 0.0);
    }

    #[test]
    fn the_inherited_velocity_is_retaken_thirty_times_a_second() {
        let (mut accum, mut held) = (0.0, Vec3::ZERO);
        let delta = Vec3::X * 0.1;
        retake_inherited_velocity(&mut accum, &mut held, 0.02, delta, true, 6.0);
        assert_eq!(held, Vec3::ZERO);
        retake_inherited_velocity(&mut accum, &mut held, 0.02, delta, true, 6.0);
        assert!((held.x - 0.5).abs() < 1e-4);
        assert_eq!(accum, 0.0);
        retake_inherited_velocity(&mut accum, &mut held, 0.02, Vec3::ZERO, true, 6.0);
        assert!((held.x - 0.5).abs() < 1e-4, "held between re-takes");
        retake_inherited_velocity(&mut accum, &mut held, 0.04, delta, false, 6.0);
        assert_eq!(held, Vec3::ZERO);
    }

    #[test]
    fn a_child_emits_once_per_parent_particle_at_that_particle() {
        let def = ParticleEmitterDef {
            flags: 0x40,
            timing: model::EmitTiming::constant(100.0),
            ..super::super::emit::tests::def(model::ParticleShape::Plane)
        };
        let mut child = RecursionEmitter {
            def,
            texture: Handle::default(),
            particles: Vec::new(),
            accumulator: 0.0,
            gate_prev: false,
            rng: 7,
        };
        let c_now = ParamsNow {
            emission_speed: 0.0,
            area_length: 0.0,
            area_width: 0.0,
            ..super::super::emit::tests::now()
        };
        let parents = [
            particle(Vec3::X * 10.0, Vec3::Y * 3.0),
            particle(Vec3::X * -10.0, Vec3::Y * -3.0),
        ];
        emit_at_parent_particles(
            &mut child,
            &c_now,
            &parents,
            100.0,
            true,
            1.0,
            0.1,
            true,
            &Transform::IDENTITY,
        );
        assert_eq!(child.particles.len(), 20);
        for p in &child.particles {
            let at_a = (p.pos - Vec3::X * 10.0).length() < 1e-3;
            let at_b = (p.pos + Vec3::X * 10.0).length() < 1e-3;
            assert!(at_a || at_b);
            let v = if at_a { Vec3::Y * 3.0 } else { Vec3::Y * -3.0 };
            assert!((p.vel - v).length() < 1e-3);
        }
    }
}
