//! An emitter-only model played once at one of a unit's attachment points: it rides the point
//! through its first sequence, and its particles live out their lives after it ends.

use bevy::animation::graph::AnimationGraphHandle;
use bevy::prelude::*;

use super::body::BodyModel;
use crate::m2::M2Model;
use crate::particles::{EmitClock, EmitterFrames, OwnerLoss, ParticleEmitter, spawn_emitter};
use crate::rig::RigPose;

pub(crate) const MOUTH: u16 = 0x11;
const CHEST: u16 = 0xf;
const BASE: u16 = 0x13;
/// Where the client hangs an effect whose attachment the host lacks, before falling back to its
/// root.
const FALLBACKS: [u16; 2] = [CHEST, BASE];
const NO_SEQUENCE_SECS: f32 = 1.0;

/// A model to play once at `attachment` on `host`. It waits for both models, then hangs itself
/// from the attachment point.
#[derive(Component)]
pub(crate) struct OneShot {
    pub(crate) model: Handle<M2Model>,
    pub(crate) host: Entity,
    attachment: u16,
    play: Play,
}

enum Play {
    Waiting,
    Playing { ends: f32, emitters: Vec<Entity> },
}

impl OneShot {
    pub(crate) fn new(model: Handle<M2Model>, host: Entity, attachment: u16) -> Self {
        Self {
            model,
            host,
            attachment,
            play: Play::Waiting,
        }
    }
}

pub(crate) fn play_one_shots(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    server: Res<'_, AssetServer>,
    m2s: Res<'_, Assets<M2Model>>,
    mut hosts: Query<'_, '_, (&BodyModel, Option<&mut RigPose>)>,
    mut shots: Query<'_, '_, (Entity, &mut OneShot)>,
    mut emitters: Query<'_, '_, &mut ParticleEmitter>,
) {
    let now = time.elapsed_secs();
    for (shot, mut one) in &mut shots {
        if let Play::Playing {
            ends,
            emitters: ref started,
        } = one.play
        {
            if now >= ends {
                for &e in started {
                    if let Ok(mut emitter) = emitters.get_mut(e) {
                        emitter.drain_on_owner_loss();
                    }
                }
                commands.entity(shot).despawn();
            }
            continue;
        }
        let Ok((body, pose)) = hosts.get_mut(one.host) else {
            commands.entity(shot).despawn();
            continue;
        };
        let (Some(model), Some(host_model)) = (m2s.get(&one.model), m2s.get(&body.0)) else {
            if server.load_state(&one.model).is_failed() {
                commands.entity(shot).despawn();
            }
            continue;
        };
        let point = std::iter::once(one.attachment)
            .chain(FALLBACKS)
            .find_map(|id| host_model.attachments.iter().find(|a| a.id == id));
        let (parent, offset) = point
            .zip(pose)
            .and_then(|(p, mut pose)| Some((pose.anchor_for(&mut commands, p.bone)?, p.offset)))
            .unwrap_or((one.host, Vec3::ZERO));
        commands.entity(shot).insert((
            Transform::from_translation(offset),
            Visibility::default(),
            ChildOf(parent),
        ));
        one.play = Play::Playing {
            ends: now + first_sequence_secs(model),
            emitters: start_emitters(&mut commands, shot, one.host, model),
        };
    }
}

fn first_sequence_secs(model: &M2Model) -> f32 {
    model
        .animations
        .as_ref()
        .and_then(|a| a.clips.iter().find(|c| c.seq_index == 0))
        .map_or(NO_SEQUENCE_SECS, |c| c.duration)
}

/// The model's emitters on its own bones, posed from its Stand, or on the shot itself when it has
/// no bones to pose.
fn start_emitters(
    commands: &mut Commands<'_, '_>,
    shot: Entity,
    host: Entity,
    model: &M2Model,
) -> Vec<Entity> {
    let anims = model.animations.as_ref();
    let clip = anims.and_then(|a| a.find(a.resolve(0, &|_| None).id).or(a.clips.first()));
    let mut pose = match (anims, clip) {
        (Some(anims), Some(clip)) if !model.skeleton.joints.is_empty() => {
            let mut player = AnimationPlayer::default();
            let playing = player.play(clip.node);
            if clip.looping {
                playing.repeat();
            }
            commands.entity(shot).insert((
                player,
                AnimationGraphHandle(anims.graph.clone()),
                anims.clone(),
            ));
            Some(RigPose::new(shot, &model.skeleton))
        }
        _ => None,
    };
    let clock = match (&pose, clip) {
        (Some(_), _) => EmitClock::Host(shot),
        (None, clip) => EmitClock::Effect(clip.map(|c| c.seq_index)),
    };
    let emitters = model
        .emitters
        .iter()
        .filter_map(|em| {
            let owner = pose
                .as_mut()
                .and_then(|p| p.anchor_for(commands, em.def.bone))
                .map_or((shot, [0.0; 3]), |joint| (joint, em.bone_pivot));
            let frames = EmitterFrames {
                owner: Some(owner),
                anchor: Some(shot),
                alpha: Some(host),
                on_owner_loss: OwnerLoss::Free,
            };
            spawn_emitter(commands, em, Transform::IDENTITY, frames, clock)
        })
        .collect();
    if let Some(pose) = pose {
        commands.entity(shot).insert(pose);
    }
    emitters
}
