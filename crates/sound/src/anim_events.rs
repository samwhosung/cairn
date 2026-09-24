//! The sound keys of animation events: a doodad's loop into the emitter pool and its stop out of
//! it, and the one-shots at the key's own point.

use bevy::prelude::*;
use world::rig_events::AnimEvent;

use crate::config::SoundConfig;
use crate::emitter_pool::AmbientEmitterPool;
use crate::kit::{KitRef, SoundCategory, SoundKits, play_kit};
use crate::{AudioListener, SoundOutput};

#[allow(clippy::too_many_arguments)]
pub(crate) fn route_anim_events(
    mut events: MessageReader<'_, '_, AnimEvent>,
    transforms: Query<'_, '_, &GlobalTransform, Without<Camera3d>>,
    kits: Option<ResMut<'_, SoundKits>>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
    mut complained: Local<'_, std::collections::HashSet<u32>>,
    mut pool: ResMut<'_, AmbientEmitterPool>,
) {
    if events.is_empty() {
        return;
    }
    let Some(mut kits) = kits else {
        events.clear();
        return;
    };
    let listener = listener.pos;
    let at = |ev: &AnimEvent| {
        ev.pos.or_else(|| {
            transforms
                .get(ev.entity)
                .ok()
                .map(GlobalTransform::translation)
        })
    };
    for ev in events.read() {
        match &ev.ident {
            b"$DSL" if ev.data != 0 => {
                if let Some(pos) = at(ev) {
                    pool.register(ev.entity, ev.data, pos, listener);
                }
            }
            b"$DSE" => pool.release(ev.entity),
            b"$SND" | b"$DSO" if ev.data != 0 => {
                if let Err(e) = play_kit(
                    &mut kits,
                    &mut out,
                    &config,
                    listener,
                    KitRef::Id(ev.data),
                    at(ev),
                    SoundCategory::Sfx,
                ) && complained.insert(ev.data)
                {
                    warn!("anim event kit {}: {e:#}", ev.data);
                }
            }
            _ => {}
        }
    }
}
