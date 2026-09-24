//! M2 particle emitters, simulated on the CPU as the client simulates them and drawn as quads on
//! the effect stream.

mod emit;
mod geometry;
#[cfg(test)]
mod install;
mod quads;
mod sim;

use bevy::prelude::*;
use geometry::GeometryInstance;
use model::ParticleEmitterDef;

use crate::m2::{M2Model, ModelEmitter};
use crate::portal::{
    CameraInteriorClaim, ExteriorGate, ExteriorWindows, WmoGroupVis, WmoPortalInstance, room_admits,
};
use crate::visibility::doodad_fade_alpha;

pub(crate) use emit::{rand_signed, rand01, xorshift32};

const MAX_PARTICLES: usize = 1024;
const MAX_RECURSION_EMITTERS: usize = 4;
pub(crate) const MAX_STEP: f32 = 0.1;

/// One live particle.
///
/// An emitter without flag `0x10` stores its particles in the world, positions and velocities
/// in Bevy's axes, baked through the emitter's placement at birth, so a moving host lays a trail;
/// after that only flag `0x4000` moves them, by its share of the host's motion. One with the flag
/// stores them in its own frame, WoW axes, and draws them through its live placement, so they
/// ride it.
#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    pub age: f32,
    /// Seconds: the lifespan the emitter sampled when it was born.
    pub life: f32,
    /// A random draw at birth that decorrelates the particles' flicker and spin.
    pub phase: u32,
    /// Not yet integrated; its first step skips the follow motion.
    pub fresh: bool,
    /// A geometry particle's orientation and spin, radians a second.
    pub quat: Quat,
    pub angvel: Vec3,
}

#[derive(Component)]
pub struct ParticleEmitter {
    def: ParticleEmitterDef,
    placement: Transform,
    owner: Option<Entity>,
    on_owner_loss: OwnerLoss,
    draining: bool,
    alpha_src: Option<Entity>,
    alpha: f32,
    anchor: Option<Entity>,
    anchor_pos: Vec3,
    particles: Vec<Particle>,
    accumulator: f32,
    emitter_prev: Option<Vec3>,
    inherit_accum: f32,
    inherit_vel: Vec3,
    gate_prev: bool,
    age: f32,
    host: Option<Entity>,
    seq: Option<usize>,
    rng: u32,
    owner_reach: f32,
    texture: Handle<Image>,
    gated: bool,
    recursion: Option<Handle<M2Model>>,
    recursion_emitters: Vec<RecursionEmitter>,
    geometry: Option<Handle<M2Model>>,
    model_instances: Vec<GeometryInstance>,
}

struct RecursionEmitter {
    def: ParticleEmitterDef,
    texture: Handle<Image>,
    particles: Vec<Particle>,
    accumulator: f32,
    gate_prev: bool,
    rng: u32,
}

#[derive(Component)]
pub struct GeometryParticleMesh;

/// What losing its owner does to an emitter.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum OwnerLoss {
    /// The owner model is gone: so is the emitter, particles and all.
    Free,
    /// The effect ended: nothing more is born, and the particles live out their lives.
    #[default]
    Drain,
}

/// The entities an emitter rides.
#[derive(Clone, Copy, Default)]
pub struct EmitterFrames {
    /// The entity whose transform places the emitter, with the bone pivot the record's position
    /// is rebased by for a joint (`[0; 3]` for a whole model); `None` for a fixed placement.
    pub owner: Option<(Entity, [f32; 3])>,
    /// The model a world-space cloud sorts at; a cloud riding its emitter sorts at the emitter.
    pub anchor: Option<Entity>,
    /// The model whose alpha fades the particles.
    pub alpha: Option<Entity>,
    pub on_owner_loss: OwnerLoss,
}

/// Which clock an emitter's tracks read.
#[derive(Clone, Copy, Default)]
pub enum EmitClock {
    /// The model's idle sequence, on the emitter's own age.
    #[default]
    Pinned,
    /// This sequence slot (the idle one when `None`), on the emitter's own age.
    Effect(Option<usize>),
    /// The sequence the host's animation player plays, at its clip time.
    Host(Entity),
}

impl ParticleEmitter {
    pub fn live(&self) -> usize {
        self.particles.len()
    }

    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn def(&self) -> &ParticleEmitterDef {
        &self.def
    }

    /// Whether the emitter is in this frame's draw set.
    pub fn in_draw_set(&self) -> bool {
        !self.gated
    }

    /// The effect this emitter belongs to is ending: let its particles live out their lives
    /// when the owner goes.
    pub fn drain_on_owner_loss(&mut self) {
        self.on_owner_loss = OwnerLoss::Drain;
    }

    fn owner_rung(&self) -> f32 {
        model::owner_last_rung(self.owner_reach)
    }
}

/// Whether a placed model is in the frame's scene, which its emitters and its animation host
/// follow: its distance fade above 0, inside the far-clip wall, in the view, seen through a window
/// to the outside when the camera is in a building, and in a room the portals show.
#[derive(Component, Clone)]
pub struct DrawSetGate {
    /// The owner's bounding sphere, world yards.
    pub radius: f32,
    pub center: Vec3,
    pub(crate) building: Option<Entity>,
    pub(crate) room: Option<WmoGroupVis>,
}

impl DrawSetGate {
    /// A placement no building holds.
    pub fn sphere(radius: f32, center: Vec3) -> Self {
        Self {
            radius,
            center,
            building: None,
            room: None,
        }
    }

    /// The owner's distance fade, which also fades its particles.
    pub fn distance_alpha(&self, cam_pos: Vec3) -> f32 {
        let (dx, dz) = (self.center.x - cam_pos.x, self.center.z - cam_pos.z);
        doodad_fade_alpha(self.radius, (dx * dx + dz * dz).sqrt())
    }

    fn in_draw_set(
        &self,
        cam_pos: Vec3,
        cam_fwd: Vec3,
        lateral_in_frustum: bool,
        exterior_admitted: bool,
        room_admitted: bool,
    ) -> bool {
        self.distance_alpha(cam_pos) > 0.0
            && crate::view::nearest_depth_within_farclip(cam_pos, cam_fwd, self.center, self.radius)
            && lateral_in_frustum
            && exterior_admitted
            && room_admitted
    }

    pub(crate) fn room_admitted(&self, instance: Option<&WmoPortalInstance>) -> bool {
        room_admits(self.room.as_ref(), instance)
    }

    pub(crate) fn exterior_admitted(
        &self,
        gate: &ExteriorGate,
        camera_building: Option<Entity>,
    ) -> bool {
        if self.building.is_some() && self.building == camera_building {
            return true;
        }
        gate.admits_sphere_box(self.center, self.radius)
    }

    pub(crate) fn admitted(&self, scene: &SceneGates<'_, '_>, view: &SceneView<'_>) -> bool {
        let lateral = view.frustum.intersects_sphere(
            &bevy::camera::primitives::Sphere {
                center: self.center.into(),
                radius: self.radius,
            },
            false,
        );
        let room = self.room_admitted(
            self.room
                .as_ref()
                .and_then(|r| scene.portals.get(r.instance).ok()),
        );
        self.in_draw_set(
            view.cam_pos,
            view.cam_fwd,
            lateral,
            self.exterior_admitted(&view.gate, view.camera_building),
            room,
        )
    }
}

#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct SceneGates<'w, 's> {
    windows: Res<'w, ExteriorWindows>,
    claim: Res<'w, CameraInteriorClaim>,
    portals: Query<'w, 's, &'static WmoPortalInstance>,
}

pub(crate) struct SceneView<'a> {
    pub(crate) cam_pos: Vec3,
    pub(crate) cam_fwd: Vec3,
    pub(crate) frustum: &'a bevy::camera::primitives::Frustum,
    gate: ExteriorGate,
    camera_building: Option<Entity>,
}

impl SceneGates<'_, '_> {
    pub(crate) fn changed(&self) -> bool {
        self.windows.is_changed() || self.claim.is_changed()
    }

    pub(crate) fn view<'a>(
        &self,
        cam: &GlobalTransform,
        projection: &bevy::camera::Projection,
        frustum: &'a bevy::camera::primitives::Frustum,
    ) -> SceneView<'a> {
        SceneView {
            cam_pos: cam.translation(),
            cam_fwd: Vec3::from(cam.forward()),
            frustum,
            gate: ExteriorGate::build(&self.windows, Some((cam, projection))),
            camera_building: self.claim.0.map(|c| c.instance),
        }
    }
}

/// Spawns an emitter for `emitter` at `placement`; `None` when it has neither a texture nor a
/// geometry model to draw with, or can never emit.
pub(crate) fn spawn_emitter(
    commands: &mut Commands<'_, '_>,
    emitter: &ModelEmitter,
    placement: Transform,
    frames: EmitterFrames,
    clock: EmitClock,
) -> Option<Entity> {
    let texture = match emitter.texture.clone() {
        Some(t) => t,
        None if emitter.geometry.is_some() => Handle::default(),
        None => return None,
    };
    let mut def = emitter.def.clone();
    let owner = frames.owner.map(|(entity, pivot)| {
        def.position = [
            def.position[0] - pivot[0],
            def.position[1] - pivot[1],
            def.position[2] - pivot[2],
        ];
        entity
    });
    if def.params.peak_lifespan() <= 0.0 || def.timing.peak_rate() <= 0.0 {
        return None;
    }
    let idle = Some(emitter.idle_seq_index);
    let (host, seq) = match clock {
        EmitClock::Pinned => (None, idle),
        EmitClock::Effect(s) => (None, s.or(idle)),
        EmitClock::Host(h) => (Some(h), idle),
    };
    let owner_reach = emitter.owner_reach * placement.scale.max_element();
    let t = placement.translation;
    let rng = (t.x.to_bits() ^ t.y.to_bits().rotate_left(11) ^ t.z.to_bits().rotate_left(22))
        .wrapping_mul(0x9E37_79B9)
        | 1;
    let mut spawned = commands.spawn((
        Transform::IDENTITY,
        ParticleEmitter {
            def,
            placement,
            owner,
            on_owner_loss: frames.on_owner_loss,
            draining: false,
            alpha_src: frames.alpha,
            alpha: 1.0,
            anchor: frames.anchor,
            anchor_pos: placement.translation,
            particles: Vec::new(),
            accumulator: 0.0,
            emitter_prev: None,
            inherit_accum: 0.0,
            inherit_vel: Vec3::ZERO,
            gate_prev: false,
            age: 0.0,
            host,
            seq,
            rng,
            owner_reach,
            texture,
            gated: false,
            recursion: emitter.recursion.clone(),
            recursion_emitters: Vec::new(),
            geometry: emitter.geometry.clone(),
            model_instances: Vec::new(),
        },
    ));
    if emitter.recursion.is_some() {
        spawned.insert(AwaitingRecursionModel);
    }
    Some(spawned.id())
}

#[derive(Component)]
struct AwaitingRecursionModel;

fn wire_child_emitters(
    mut commands: Commands<'_, '_>,
    models: Res<'_, Assets<M2Model>>,
    mut emitters: Query<'_, '_, (Entity, &mut ParticleEmitter), With<AwaitingRecursionModel>>,
) {
    for (entity, mut emitter) in &mut emitters {
        let Some(model) = emitter.recursion.as_ref().and_then(|h| models.get(h)) else {
            continue;
        };
        let mut rng_seed = emitter.rng.rotate_left(7) | 1;
        let recursion_emitters: Vec<RecursionEmitter> = model
            .emitters
            .iter()
            .take(MAX_RECURSION_EMITTERS)
            .filter_map(|em| {
                let texture = em.texture.clone()?;
                if em.def.params.peak_lifespan() <= 0.0 || em.def.timing.peak_rate() <= 0.0 {
                    return None;
                }
                Some(RecursionEmitter {
                    def: em.def.clone(),
                    texture,
                    particles: Vec::new(),
                    accumulator: 0.0,
                    gate_prev: false,
                    rng: {
                        rng_seed = rng_seed.wrapping_mul(0x9E37_79B9) | 1;
                        rng_seed
                    },
                })
            })
            .collect();
        emitter.recursion = None;
        emitter.recursion_emitters = recursion_emitters;
        commands.entity(entity).remove::<AwaitingRecursionModel>();
    }
}

fn accumulate_emission(
    is_burst: bool,
    rate: f32,
    emitting: bool,
    scale: f32,
    dt: f32,
    accumulator: &mut f32,
    gate_prev: &mut bool,
) -> f32 {
    if !emitting {
        *accumulator = 0.0;
    }
    let rate = if emitting { rate.max(0.0) } else { 0.0 };
    let gate = rate > 0.0;
    let mut burst = 0.0;
    if is_burst {
        if gate && !*gate_prev {
            burst = (rate * scale).trunc();
            *accumulator = burst;
        }
    } else if gate {
        *accumulator += rate * scale * dt;
    }
    *gate_prev = gate;
    burst
}

pub(crate) struct ParticlePlugin;

impl Plugin for ParticlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<geometry::GeometryMeshes>().add_systems(
            PostUpdate,
            (
                wire_child_emitters,
                sim::simulate_particles,
                geometry::update_model_particles,
            )
                .chain()
                .after(crate::effects::begin_effect_frame)
                .after(crate::rig::RigFinalize)
                .after(crate::billboard::face_billboards),
        );
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_burst_fires_once_as_its_gate_rises() {
        let (mut acc, mut prev) = (0.0, false);
        assert_eq!(
            accumulate_emission(true, 0.0, true, 1.0, 0.016, &mut acc, &mut prev),
            0.0
        );
        assert_eq!(
            accumulate_emission(true, 30.0, true, 1.0, 0.016, &mut acc, &mut prev),
            30.0
        );
        acc = 0.0;
        accumulate_emission(true, 30.0, true, 1.0, 0.016, &mut acc, &mut prev);
        assert_eq!(acc, 0.0, "a held gate never pours");
        accumulate_emission(true, 30.0, false, 1.0, 0.016, &mut acc, &mut prev);
        assert_eq!(
            accumulate_emission(true, 30.0, true, 0.55, 0.016, &mut acc, &mut prev),
            16.0
        );
    }

    #[test]
    fn a_pouring_emitter_owes_rate_times_dt() {
        let (mut acc, mut prev) = (0.0, false);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        accumulate_emission(false, 30.0, true, 1.0, 0.1, &mut acc, &mut prev);
        assert!((acc - 6.0).abs() < 1e-4);
        accumulate_emission(false, 30.0, false, 1.0, 0.1, &mut acc, &mut prev);
        assert_eq!(acc, 0.0, "a closed gate forgets what it owed");
    }

    fn gate(radius: f32, depth: f32) -> bool {
        DrawSetGate::sphere(radius, Vec3::new(0.0, 0.0, -depth)).in_draw_set(
            Vec3::ZERO,
            Vec3::NEG_Z,
            true,
            true,
            true,
        )
    }

    #[test]
    fn a_sealed_room_keeps_its_own_props_and_stops_the_rest() {
        let mut w = World::new();
        let (here, elsewhere) = (w.spawn_empty().id(), w.spawn_empty().id());
        let sealed = ExteriorGate::Windows(Vec::new());
        let fade = |building| DrawSetGate {
            building,
            ..DrawSetGate::sphere(2.0, Vec3::new(0.0, 0.0, -60.0))
        };
        assert!(fade(Some(here)).exterior_admitted(&sealed, Some(here)));
        assert!(!fade(Some(elsewhere)).exterior_admitted(&sealed, Some(here)));
        assert!(!fade(None).exterior_admitted(&sealed, Some(here)));
        assert!(!fade(None).exterior_admitted(&sealed, None));
        for building in [Some(here), Some(elsewhere), None] {
            assert!(fade(building).exterior_admitted(&ExteriorGate::Open, Some(here)));
        }
    }

    #[test]
    fn a_big_owner_stops_at_the_wall_and_a_small_one_at_its_fade() {
        assert!(gate(8.0, 300.0));
        assert!(!gate(8.0, 400.0), "past the far-clip wall");
        assert!(gate(0.3, 45.0));
        assert!(!gate(0.3, 60.0), "past a small prop's fade");
    }
}
