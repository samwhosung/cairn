//! A placed doodad whose idle sequences carry a sound key runs its sequence's clock whether or not
//! anything on it moves, rolls a new variation every play window, and fires the keys it crosses
//! while it is drawn. A doodad out of the frame's draw set scans nothing, and one coming back
//! re-arms rather than replaying what it missed.

use bevy::animation::graph::AnimationNodeIndex;
use bevy::camera::primitives::{Frustum, Sphere};
use bevy::prelude::*;

use crate::portal::{WmoGroupVis, WmoPortalInstance, room_admits};
use crate::rig::{AnimRng, ModelAnimations};
use crate::rig_events::{AnimEvent, EventFrame, TrackMemory, advance_track, scan_events};
use crate::view::{FARCLIP, WorldCamera};
use crate::visibility::doodad_fade_alpha;

/// The keys that make a placed doodad sound: the looping emitter, its stop, and two one-shots.
const SOUND_EVENT_TAGS: [&[u8; 4]; 4] = [b"$DSL", b"$DSO", b"$DSE", b"$SND"];

/// Whether the idle sequence's variations carry a sound key.
pub(crate) fn arms_for_sound(anims: &ModelAnimations) -> bool {
    let Some(idle) = anims.idle_clip() else {
        return false;
    };
    anims
        .clips
        .iter()
        .filter(|c| c.anim_id == idle.anim_id)
        .any(|c| {
            c.events
                .iter()
                .any(|e| SOUND_EVENT_TAGS.contains(&&e.ident))
        })
}

/// A sounding doodad's clock, at its placement.
#[derive(Component)]
pub struct SoundHost {
    /// The placement's drawn parts; the host is in the draw set while any is shown.
    meshes: Vec<Entity>,
    /// A model with no mesh is in the draw set by its fade sphere, world space.
    fade: (Vec3, f32),
    room: Option<WmoGroupVis>,
    clip: Option<(AnimationNodeIndex, f32)>,
    anim_id: Option<u16>,
    armed_at: f32,
    /// When the armed window ends; born expired, so the first frame rolls the first arm.
    window_hi: f32,
    active: bool,
}

impl SoundHost {
    pub(crate) fn new(
        anims: &ModelAnimations,
        meshes: Vec<Entity>,
        fade: (Vec3, f32),
        room: Option<WmoGroupVis>,
    ) -> Option<Self> {
        let head = anims.idle_clip()?;
        Some(Self {
            meshes,
            fade,
            room,
            clip: Some((head.node, head.duration)),
            anim_id: Some(head.anim_id),
            armed_at: 0.0,
            window_hi: f32::NEG_INFINITY,
            active: false,
        })
    }

    /// The armed clip and how far into it the shared clock stands.
    fn arm_clock(&self, now: f32) -> Option<(AnimationNodeIndex, f32)> {
        let (node, duration) = self.clip?;
        let seek = if duration > 0.0 {
            (now - self.armed_at).rem_euclid(duration)
        } else {
            0.0
        };
        Some((node, seek))
    }
}

/// When an armed window ends, a new frequency-weighted variation of the same sequence and the
/// passes it plays: every host, drawn or not.
pub(crate) fn reroll_sound_hosts(
    time: Res<'_, Time>,
    mut rng: ResMut<'_, AnimRng>,
    mut hosts: Query<'_, '_, (&mut SoundHost, &ModelAnimations)>,
) {
    let now = time.elapsed_secs();
    for (mut host, anims) in &mut hosts {
        let Some(anim_id) = host.anim_id else {
            continue;
        };
        if now < host.window_hi {
            continue;
        }
        let Some(clip) = anims.pick_variation(anim_id, rng.draw()) else {
            host.anim_id = None;
            continue;
        };
        let (node, duration) = (clip.node, clip.duration);
        let replay = rng.replay_count(clip.replay);
        host.armed_at = now;
        host.window_hi = now + (duration * replay as f32).max(f32::EPSILON);
        host.clip = Some((node, duration));
    }
}

/// Whether each host is in the frame's draw set.
pub(crate) fn gate_sound_hosts(
    mut hosts: Query<'_, '_, &mut SoundHost>,
    vis: Query<'_, '_, &Visibility>,
    camera: Query<'_, '_, (&GlobalTransform, &Frustum), With<WorldCamera>>,
    instances: Query<'_, '_, &WmoPortalInstance>,
) {
    let cam = camera.single().ok();
    for mut host in &mut hosts {
        let drawn = if host.meshes.is_empty() {
            let (center, radius) = host.fade;
            cam.is_some_and(|(cam, frustum)| {
                let pos = cam.translation();
                let (dx, dz) = (center.x - pos.x, center.z - pos.z);
                let instance = host
                    .room
                    .as_ref()
                    .and_then(|r| instances.get(r.instance).ok());
                doodad_fade_alpha(radius, (dx * dx + dz * dz).sqrt()) > 0.0
                    && (center - pos).dot(*cam.forward()) - radius <= FARCLIP
                    && frustum.intersects_sphere(
                        &Sphere {
                            center: center.into(),
                            radius,
                        },
                        false,
                    )
                    && room_admits(host.room.as_ref(), instance)
            })
        } else {
            host.meshes
                .iter()
                .any(|&e| vis.get(e).is_ok_and(|v| *v != Visibility::Hidden))
        };
        if host.active != drawn {
            host.active = drawn;
        }
    }
}

/// Fires the keys each drawn host's armed clip crossed this frame, at their model-space points.
pub(crate) fn fire_sound_host_events(
    time: Res<'_, Time>,
    hosts: Query<'_, '_, (Entity, &SoundHost, &ModelAnimations, &GlobalTransform)>,
    mut last: Local<'_, TrackMemory>,
    mut out: MessageWriter<'_, AnimEvent>,
) {
    let now = time.elapsed_secs();
    for (entity, host, anims, world) in &hosts {
        if !host.active {
            last.remove(&entity);
            continue;
        }
        let Some((node, cur)) = host.arm_clock(now) else {
            continue;
        };
        let Some(clip) = anims.clips.iter().find(|c| c.node == node) else {
            continue;
        };
        if let Some(prev) = advance_track(&mut last, entity, node, cur) {
            let frame = EventFrame { world, rig: None };
            scan_events(clip, entity, prev, cur, &frame, &mut out);
        }
    }
    last.retain(|e, _| hosts.contains(*e));
}
