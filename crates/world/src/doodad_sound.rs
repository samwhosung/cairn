//! A drawn doodad fires the sound keys its armed clip crosses. The client scans only what it
//! draws, so a doodad coming back into the frame re-arms rather than replaying what it missed.

use bevy::prelude::*;

use crate::doodad_anim::DoodadAnimHost;
use crate::rig::{ModelAnimations, RigPose};
use crate::rig_events::{AnimEvent, EventFrame, TrackMemory, advance_track, scan_events};

/// The keys that make a placed doodad sound: the looping emitter, its stop, and two one-shots.
const SOUND_EVENT_TAGS: [&[u8; 4]; 4] = [b"$DSL", b"$DSE", b"$DSO", b"$SND"];

pub(crate) fn idle_has_sound_keys(anims: &ModelAnimations) -> bool {
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

type Hosts<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static DoodadAnimHost,
        &'static ModelAnimations,
        &'static GlobalTransform,
        Option<&'static RigPose>,
    ),
>;

pub(crate) fn fire_doodad_events(
    time: Res<'_, Time>,
    hosts: Hosts<'_, '_>,
    globals: Query<'_, '_, &GlobalTransform>,
    mut last: Local<'_, TrackMemory>,
    mut out: MessageWriter<'_, AnimEvent>,
) {
    let now = time.elapsed_secs();
    for (entity, host, anims, world, pose) in &hosts {
        if !host.gate.posing() {
            last.remove(&entity);
            continue;
        }
        let Some((node, cur)) = host.arm_clock(now) else {
            continue;
        };
        let Some(clip) = anims.clips.iter().find(|c| c.node == node) else {
            continue;
        };
        let scan = advance_track(&mut last, entity, node, cur);
        let frame = EventFrame {
            world,
            rig: pose.and_then(|p| Some((p, globals.get(p.joints_root).ok()?))),
        };
        scan_events(clip, entity, scan, cur, &frame, &mut out);
    }
    last.retain(|e, _| hosts.contains(*e));
}
