use bevy::prelude::*;
use kira::sound::PlaybackState;

use super::mixer;
use crate::config::SoundConfig;
use crate::{AudioListener, SoundOutput, math};

/// The client virtualises a channel past its cutoff; this stops it, and its trigger restarts it.
#[allow(
    clippy::float_cmp,
    reason = "an unchanged amp is bit-identical to the last frame's"
)]
pub fn pump_channels(
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    listener: Res<'_, AudioListener>,
    transforms: Query<'_, '_, &Transform, Without<Camera3d>>,
) {
    let listener = listener.pos;
    out.channels.retain_mut(|ch| {
        if ch.handle.state() == PlaybackState::Stopped {
            return false;
        }
        if ch.tracked
            && let Some(p) = ch
                .source
                .and_then(|s| transforms.get(s).ok())
                .map(|t| t.translation)
            && ch.pos != Some(p)
        {
            ch.pos = Some(p);
            if let Some(track) = ch.track.as_mut() {
                mixer::set_track_position(track, p);
            }
        }
        // Writes only when the amp moved: a volume write is a command into the audio thread
        // whether or not the value changed.
        let Some(p) = ch.pos else {
            let amp = config.category_amp(ch.category) * ch.shot_volume * ch.fade_gain;
            if amp != ch.fed_amp {
                ch.fed_amp = amp;
                ch.handle
                    .set_volume(mixer::amp_to_db(ch.fed_amp), mixer::glide());
            }
            return true;
        };
        let d_sq = math::dist_sq(listener, p);
        if ch.cutoff > 0.0 && !math::audible(d_sq, ch.cutoff) {
            ch.handle.stop(mixer::declick());
            return false;
        }
        let amp = config.category_amp(ch.category)
            * ch.shot_volume
            * ch.fade_gain
            * math::fmod_rolloff(d_sq, ch.min_dist)
            * super::near_field(d_sq, ch.cutoff);
        if amp != ch.fed_amp {
            ch.fed_amp = amp;
            ch.handle
                .set_volume(mixer::amp_to_db(ch.fed_amp), mixer::glide());
        }
        true
    });
}

pub(crate) fn source_kit_playing(out: &SoundOutput, source: Entity, kit_id: u32) -> bool {
    out.channels
        .iter()
        .any(|c| c.source == Some(source) && c.kit == kit_id)
}

/// The pump multiplies this in on its next run.
pub(crate) fn set_source_kit_gain(out: &mut SoundOutput, source: Entity, kit_id: u32, gain: f32) {
    for c in &mut out.channels {
        if c.source == Some(source) && c.kit == kit_id {
            c.fade_gain = gain.clamp(0.0, 1.0);
        }
    }
}

pub(crate) fn stop_source_kit(out: &mut SoundOutput, source: Entity, kit_id: u32) {
    out.channels.retain_mut(|c| {
        if c.source == Some(source) && c.kit == kit_id {
            c.handle.stop(mixer::declick());
            false
        } else {
            true
        }
    });
}

pub(crate) fn stop_source(out: &mut SoundOutput, source: Entity) {
    out.channels.retain_mut(|c| {
        if c.source == Some(source) {
            c.handle.stop(mixer::declick());
            false
        } else {
            true
        }
    });
}
