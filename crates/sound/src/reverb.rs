//! The zone's reverb preset on the mixer's reverb send.

use bevy::prelude::*;
use world::interior::CurrentArea;
use world::submersion::Underwater;

use crate::SoundOutput;
use crate::config::SoundConfig;
use crate::interior::CurrentInterior;
use crate::tables::{AreaSounds, SoundProviders};

#[derive(Resource, Default)]
pub(crate) struct AppliedPreset(Option<u32>);

#[allow(clippy::too_many_arguments)]
pub(crate) fn zone_reverb(
    mut applied: ResMut<'_, AppliedPreset>,
    mut out: NonSendMut<'_, SoundOutput>,
    underwater: Res<'_, Underwater>,
    area: Res<'_, CurrentArea>,
    areas: Option<Res<'_, AreaSounds>>,
    providers: Option<Res<'_, SoundProviders>>,
    config: Res<'_, SoundConfig>,
    interior: Res<'_, CurrentInterior>,
) {
    let (Some(areas), Some(providers)) = (areas, providers) else {
        return;
    };
    let column = usize::from(underwater.0.is_water());
    let pref = if config.enabled && config.reverb {
        interior
            .0
            .map(|i| i.sound_provider[column])
            .filter(|p| *p != 0)
            .or_else(|| {
                area.0
                    .and_then(|id| areas.resolve(id))
                    .map(|a| a.sound_provider[column])
            })
            .unwrap_or(0)
    } else {
        0
    };
    if applied.0 == Some(pref) {
        return;
    }
    let Some(mixer) = out.mixer.as_mut() else {
        return;
    };
    let preset = (pref != 0).then(|| providers.get(pref)).flatten();
    if pref != 0 && preset.is_none() {
        warn!("reverb: unknown preset {pref}");
    }
    if let Some(p) = preset {
        info!("reverb: {} (decay {:.2} s)", p.name, p.decay_time);
    } else if applied.0.is_some_and(|p| p != 0) {
        info!("reverb: off");
    }
    mixer.set_reverb(preset);
    applied.0 = Some(pref);
}
