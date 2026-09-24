mod lazy;
mod mat_anim;
mod placement;

use bevy::animation::graph::{AnimationGraphHandle, AnimationNodeIndex};
use bevy::app::AnimationSystems;
use bevy::camera::primitives::Frustum;
use bevy::prelude::*;

use lazy::LazyRig;
use mat_anim::TintAnimMaterials;
pub(crate) use mat_anim::{AnimMatPart, MatAnim, MatLoop, UvAnimMaterials, register};
pub(crate) use placement::{MaterialLoops, RigBuilder};

use crate::doodad_events::idle_has_sound_keys;
use crate::m2::M2Model;
use crate::particles::{DrawSetGate, SceneGates};
use crate::portal::WmoPortalInstance;
use crate::rig::{
    AnimClip, AnimParked, AnimRng, GlobalSeqDrive, ModelAnimations, ModelSkeleton, RigPalettes,
    RigPose, RigSkin,
};
use crate::view::WorldCamera;
use crate::visibility::{ModelPart, apply_model_visibility};

pub(crate) enum DoodadAnimTier<'a> {
    Static,
    GlobalSeqOnly,
    MovingIdle(&'a AnimClip),
}

pub(crate) fn classify<'a>(
    skeleton: &ModelSkeleton,
    animations: Option<&'a ModelAnimations>,
) -> DoodadAnimTier<'a> {
    let Some(anims) = animations else {
        return DoodadAnimTier::Static;
    };
    if skeleton.joints.is_empty() {
        return DoodadAnimTier::Static;
    }
    match anims.moving_idle.and_then(|i| anims.clips.get(i)) {
        Some(clip) => DoodadAnimTier::MovingIdle(clip),
        None if !anims.global_bones.is_empty() => DoodadAnimTier::GlobalSeqOnly,
        None => DoodadAnimTier::Static,
    }
}

enum Arm<'a> {
    Posed(&'a AnimClip),
    ClockOnly(&'a AnimClip),
    GlobalSeqsOnly,
}

fn arm<'a>(skeleton: &ModelSkeleton, anims: &'a ModelAnimations) -> Option<Arm<'a>> {
    let tier = classify(skeleton, Some(anims));
    if let DoodadAnimTier::MovingIdle(idle) = tier {
        return Some(Arm::Posed(idle));
    }
    match (
        tier,
        anims.idle_clip().filter(|_| idle_has_sound_keys(anims)),
    ) {
        (_, Some(idle)) => Some(Arm::ClockOnly(idle)),
        (DoodadAnimTier::GlobalSeqOnly, None) => Some(Arm::GlobalSeqsOnly),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArmedClip {
    pub(crate) node: AnimationNodeIndex,
    pub(crate) duration: f32,
}

pub(crate) struct HostBuilder {
    pub(crate) root: Entity,
    pose: RigPose,
    clip: Option<ArmedClip>,
    anim_id: Option<u16>,
    skins: bool,
    pub(crate) plays_idle_on_player: bool,
}

impl HostBuilder {
    pub(crate) fn anchor(&mut self, commands: &mut Commands<'_, '_>, bone: u16) -> Option<Entity> {
        self.pose.anchor_for(commands, bone)
    }

    fn finish(self, commands: &mut Commands<'_, '_>) {
        commands.entity(self.root).insert(self.pose);
    }
}

pub(crate) fn spawn_anim_host(
    commands: &mut Commands<'_, '_>,
    m: &M2Model,
    transform: Transform,
) -> Option<HostBuilder> {
    let anims = m.animations.as_ref()?;
    let arm = arm(&m.skeleton, anims)?;
    let root = commands.spawn((transform, Visibility::default())).id();
    let pose = RigPose::new(root, &m.skeleton);
    let skins = !matches!(classify(&m.skeleton, Some(anims)), DoodadAnimTier::Static);
    let plays_idle_on_player = matches!(arm, Arm::Posed(_));
    let idle = match arm {
        Arm::Posed(idle) => {
            let mut player = AnimationPlayer::default();
            player.play(idle.node).repeat();
            commands.entity(root).insert((
                player,
                AnimationGraphHandle(anims.graph.clone()),
                anims.clone(),
            ));
            Some(idle)
        }
        Arm::ClockOnly(idle) => {
            commands.entity(root).insert(anims.clone());
            Some(idle)
        }
        Arm::GlobalSeqsOnly => None,
    };
    if let Some(drive) = GlobalSeqDrive::new(&anims.global_bones, m.skeleton.joints.len()) {
        commands.entity(root).insert(drive);
    }
    Some(HostBuilder {
        root,
        pose,
        clip: idle.map(|c| ArmedClip {
            node: c.node,
            duration: c.duration,
        }),
        anim_id: idle.map(|c| c.anim_id),
        skins,
        plays_idle_on_player,
    })
}

pub(crate) enum SeenBy {
    Batches(Vec<Entity>),
    Bounds(DrawSetGate),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    New,
    ArmedAtSpawn,
    Drawn,
    Parked,
}

impl Gate {
    pub(crate) fn posing(self) -> bool {
        matches!(self, Gate::ArmedAtSpawn | Gate::Drawn)
    }
}

#[derive(Component)]
pub struct DoodadAnimHost {
    pub(crate) seen_by: SeenBy,
    pub(crate) clip: Option<ArmedClip>,
    pub(crate) armed_at: f32,
    pub(crate) rerolls_at: f32,
    pub(crate) anim_id: Option<u16>,
    pub(crate) gate: Gate,
    pub(crate) parked_at: f32,
    pub(crate) own_stream: Option<AnimRng>,
}

impl DoodadAnimHost {
    pub(crate) fn arm_clock(&self, now: f32) -> Option<(AnimationNodeIndex, f32)> {
        let clip = self.clip?;
        let seek = if clip.duration > 0.0 {
            (now - self.armed_at).rem_euclid(clip.duration)
        } else {
            0.0
        };
        Some((clip.node, seek))
    }
}

fn reroll_doodad_variation(
    time: Res<'_, Time>,
    session: Res<'_, AnimRng>,
    mut hosts: Query<
        '_,
        '_,
        (
            &mut DoodadAnimHost,
            &ModelAnimations,
            &Transform,
            Option<&mut AnimationPlayer>,
        ),
    >,
) {
    let now = time.elapsed_secs();
    for (mut host, anims, at, player) in &mut hosts {
        let Some(anim_id) = host.anim_id else {
            continue;
        };
        if now < host.rerolls_at {
            continue;
        }
        let mut rng = host
            .own_stream
            .unwrap_or_else(|| session.at(at.translation));
        let Some(clip) = anims.pick_variation(anim_id, rng.draw()) else {
            host.anim_id = None;
            continue;
        };
        let armed = ArmedClip {
            node: clip.node,
            duration: clip.duration,
        };
        let replay = rng.replay_count(clip.replay);
        host.own_stream = Some(rng);
        host.armed_at = now;
        host.rerolls_at = now + (armed.duration * replay as f32).max(f32::EPSILON);
        host.clip = Some(armed);
        if host.gate.posing()
            && let Some(mut p) = player
        {
            // The client snaps to the new variation rather than blending into it.
            p.stop_all();
            p.play(armed.node).repeat();
        }
    }
}

type Hosts<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut DoodadAnimHost,
        Option<&'static LazyRig>,
        Option<&'static RigPose>,
        Has<RigSkin>,
        Option<&'static mut AnimationPlayer>,
    ),
>;

type Camera<'w, 's> = Query<
    'w,
    's,
    (
        Ref<'static, GlobalTransform>,
        &'static Frustum,
        Ref<'static, Projection>,
        Option<Ref<'static, Transform>>,
    ),
    With<WorldCamera>,
>;

#[allow(clippy::too_many_arguments)]
fn gate_doodad_anim(
    time: Res<'_, Time>,
    mut hosts: Hosts<'_, '_>,
    vis: Query<'_, '_, &Visibility>,
    cam: Camera<'_, '_>,
    changed_vis: Query<'_, '_, (), (Changed<Visibility>, With<ModelPart>)>,
    gates: SceneGates<'_, '_>,
    changed_instances: Query<'_, '_, (), Changed<WmoPortalInstance>>,
    mut palettes: ResMut<'_, RigPalettes>,
    worlds: Query<'_, '_, &GlobalTransform>,
    mut twin_parts: lazy::TwinParts<'_, '_>,
    mut commands: Commands<'_, '_>,
) {
    let now = time.elapsed_secs();
    let world_cam = cam.single().ok();
    let still = world_cam.as_ref().is_some_and(|(tf, _, proj, local)| {
        !tf.is_changed()
            && !proj.is_changed()
            && !local.as_ref().is_some_and(DetectChanges::is_changed)
    }) && changed_vis.is_empty()
        && changed_instances.is_empty()
        && !gates.changed();
    let view = world_cam
        .as_ref()
        .map(|(tf, frustum, proj, _)| gates.view(tf, proj, frustum));
    for (entity, mut host, lazy, pose, has_rig, player) in &mut hosts {
        let was_posing = host.gate.posing();
        let drawn = match host.gate {
            Gate::New => true,
            Gate::Drawn | Gate::Parked if still => was_posing,
            _ => match &host.seen_by {
                SeenBy::Batches(batches) => batches
                    .iter()
                    .any(|&e| vis.get(e).is_ok_and(|v| *v != Visibility::Hidden)),
                SeenBy::Bounds(fade) => view.as_ref().is_some_and(|v| fade.admitted(&gates, v)),
            },
        };
        if drawn
            && was_posing
            && !has_rig
            && let Some(lazy) = lazy
        {
            lazy::promote_lazy_rig(
                &mut commands,
                &mut palettes,
                &worlds,
                entity,
                lazy,
                pose,
                &mut twin_parts,
            );
        }
        let gate = match (host.gate, drawn) {
            (Gate::New, _) => Gate::ArmedAtSpawn,
            (_, true) => Gate::Drawn,
            (_, false) => Gate::Parked,
        };
        if host.gate != gate {
            host.gate = gate;
        }
        if drawn == was_posing {
            continue;
        }
        if drawn {
            commands.entity(entity).remove::<AnimParked>();
        } else {
            host.parked_at = now;
            commands.entity(entity).insert(AnimParked);
        }
        let Some(mut p) = player else {
            continue;
        };
        if !drawn {
            p.stop_all();
        } else if let Some(clip) = host.clip {
            let anim = p.start(clip.node);
            anim.repeat();
            if clip.duration > 0.0 {
                anim.seek_to((now - host.armed_at).rem_euclid(clip.duration));
            }
        }
    }
}

pub(crate) struct DoodadAnimPlugin;

impl Plugin for DoodadAnimPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UvAnimMaterials>()
            .init_resource::<TintAnimMaterials>()
            .add_systems(
                PostUpdate,
                (reroll_doodad_variation, gate_doodad_anim)
                    .chain()
                    .before(AnimationSystems),
            )
            .add_systems(
                Update,
                (
                    lazy::reap_parked_rigs,
                    mat_anim::sample_mat_anim.before(apply_model_visibility),
                    mat_anim::tick_anim_materials.after(apply_model_visibility),
                ),
            );
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests;
