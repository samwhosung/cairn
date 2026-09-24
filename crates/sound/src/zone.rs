//! Zone music and ambience, from the area the player stands in and the room over it.
//!
//! Music: a change of the zone's music row fades the playing track out over 4 s while the new
//! row's track starts at once, at full volume; within one zone, tracks are separated by a random
//! silence from the row. An entry fanfare takes the slot first when its throttle allows.
//!
//! Ambience: the bed is the submerged loop under water, else the room's, else the area's, by day
//! or by night. A change crossfades over 5 s, except going under or coming up, which is instant.
//! Day is 05:30 to 21:00.

use std::collections::HashMap;

use bevy::prelude::*;
use kira::sound::FromFileError;
use world::TimeOfDay;
use world::interior::CurrentArea;
use world::submersion::Underwater;

use crate::SoundOutput;
use crate::config::SoundConfig;
use crate::interior::{CurrentInterior, InteriorAudio};
use crate::kit::{SoundCategory, SoundKits};
use crate::mixer::{self, StaticSoundHandle, StreamWatch, StreamingSoundHandle};
use crate::tables::AreaSounds;

/// `UnderWaterLoop`.
const UNDERWATER_LOOP_KIT: u32 = 4123;
const MUSIC_FADE_OUT_MS: u64 = 4000;
const AMBIENCE_TRANSITION_FADE_MS: u64 = 5000;
/// Without a row to say how long, the silence before the next track.
const DEFAULT_SILENCE_SECS: f64 = 6.0;

/// 0 by day, 1 by night: the index into the tables' day and night pairs.
fn phase(time: TimeOfDay) -> usize {
    usize::from(!(330..1260).contains(&time.minute))
}

/// The incoming leg of an ambience crossfade, ramped every frame: a handle-level fade would be
/// overwritten by the per-frame volume feed.
#[derive(Clone, Copy)]
struct FadeIn {
    start: f64,
    dur: f64,
}

impl FadeIn {
    fn gain(self, now: f64) -> f32 {
        if self.dur <= 0.0 {
            return 1.0;
        }
        (((now - self.start) / self.dur) as f32).clamp(0.0, 1.0)
    }
}

fn fade_in_gain(fade: &mut Option<FadeIn>, now: f64) -> f32 {
    match fade {
        Some(f) => {
            let g = f.gain(now);
            if g >= 1.0 {
                *fade = None;
            }
            g
        }
        None => 1.0,
    }
}

/// The two slots and their schedule.
pub(crate) struct ZoneAudio {
    zone_music: u32,
    music: Option<StreamingSoundHandle<FromFileError>>,
    music_watch: StreamWatch,
    music_kit_vol: f32,
    /// When the next track starts; `None` while one plays or the zone has none.
    next_track_at: Option<f64>,
    ambience_kit: u32,
    ambience: Option<StaticSoundHandle>,
    ambience_kit_vol: f32,
    ambience_fade_in: Option<FadeIn>,
    /// The fanfare throttle: row id to when it last played.
    intro_last: HashMap<u32, f64>,
    area: Option<u32>,
    interior: Option<InteriorAudio>,
    was_underwater: bool,
    rng: u32,
}

impl Default for ZoneAudio {
    fn default() -> Self {
        Self {
            zone_music: 0,
            music: None,
            music_watch: StreamWatch::new("zone music"),
            music_kit_vol: 1.0,
            next_track_at: None,
            ambience_kit: 0,
            ambience: None,
            ambience_kit_vol: 1.0,
            ambience_fade_in: None,
            intro_last: HashMap::new(),
            area: None,
            interior: None,
            was_underwater: false,
            rng: 0x1234_5677,
        }
    }
}

impl ZoneAudio {
    fn rand(&mut self) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x
    }

    fn silence_ms(&mut self, min: u32, max: u32) -> u32 {
        if max > min {
            min + self.rand() % (max - min + 1)
        } else {
            min
        }
    }
}

pub(crate) fn load_area_sounds(
    mut commands: Commands<'_, '_>,
    install: Option<Res<'_, world::Install>>,
) {
    let Some(install) = install else { return };
    match AreaSounds::load(&install.0) {
        Ok(areas) => {
            info!("sound: {} areas", areas.len());
            commands.insert_resource(areas);
        }
        Err(e) => warn!("sound: no zone music or ambience: {e}"),
    }
}

/// What the world under the player says this frame.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Place<'w> {
    area: Res<'w, CurrentArea>,
    interior: Res<'w, CurrentInterior>,
    underwater: Res<'w, Underwater>,
    time_of_day: Res<'w, TimeOfDay>,
}

#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    reason = "one scheduler, in the client's own order"
)]
pub(crate) fn zone_audio(
    mut zone: NonSendMut<'_, ZoneAudio>,
    mut out: NonSendMut<'_, SoundOutput>,
    areas: Option<Res<'_, AreaSounds>>,
    kits: Option<ResMut<'_, SoundKits>>,
    config: Res<'_, SoundConfig>,
    place: Place<'_>,
    time: Res<'_, Time>,
    real: Res<'_, Time<bevy::time::Real>>,
) {
    let (Some(areas), Some(mut kits)) = (areas, kits) else {
        return;
    };
    let now = time.elapsed_secs_f64();
    let phase = phase(*place.time_of_day);
    let zone = &mut *zone;
    let inside = place.interior.0;
    let area = place.area.0;
    if area != zone.area || inside != zone.interior {
        zone.area = area;
        zone.interior = inside;
        let resolved = area.and_then(|id| areas.resolve(id));
        let (mut music_row, mut intro) = match &resolved {
            Some(r) => (r.music.map_or(0, |m| m.id), r.intro.copied()),
            None => (0, None),
        };
        if let Some(i) = inside {
            if i.zone_music != 0 {
                music_row = i.zone_music;
            }
            if i.intro_sound != 0 {
                intro = areas.intro(i.intro_sound).copied();
            }
        }
        if music_row != zone.zone_music {
            zone.zone_music = music_row;
            if let Some(mut h) = zone.music.take() {
                h.stop(mixer::fade(MUSIC_FADE_OUT_MS));
            }
            zone.next_track_at = None;
            let mut slot_taken = false;
            if let Some(intro) = intro {
                let ok_at = zone
                    .intro_last
                    .get(&intro.id)
                    .map(|t| t + f64::from(intro.min_delay_minutes) * 60.0);
                if ok_at.is_none_or(|t| now >= t)
                    && intro.sound_id != 0
                    && start_music_stream(zone, &mut out, &mut kits, &config, intro.sound_id)
                {
                    zone.intro_last.insert(intro.id, now);
                    slot_taken = true;
                }
            }
            if !slot_taken
                && let Some(kit) = areas.zone_music(music_row).map(|m| m.sounds[phase])
                && kit != 0
            {
                start_music_stream(zone, &mut out, &mut kits, &config, kit);
            }
        }
    }

    let submerged = place.underwater.0.is_water();
    let desired = if submerged {
        UNDERWATER_LOOP_KIT
    } else if let Some(kit) = zone
        .interior
        .filter(|i| i.ambience != 0)
        .and_then(|i| areas.ambience(i.ambience))
        .map(|kits| kits[phase])
        .filter(|k| *k != 0)
    {
        kit
    } else {
        zone.area
            .and_then(|id| areas.resolve(id))
            .and_then(|r| r.ambience.map(|kits| kits[phase]))
            .unwrap_or(0)
    };
    if desired != zone.ambience_kit {
        let fade_ms = if submerged == zone.was_underwater {
            AMBIENCE_TRANSITION_FADE_MS
        } else {
            0
        };
        swap_ambience(zone, &mut out, &mut kits, &config, desired, fade_ms, now);
    }
    zone.was_underwater = submerged;

    if zone
        .music
        .as_ref()
        .is_some_and(|h| h.state() == kira::sound::PlaybackState::Stopped)
    {
        zone.music = None;
        let row = zone.zone_music;
        zone.next_track_at = next_track_time(zone, &areas, row, phase, now, &config);
    }
    if zone.next_track_at.is_some_and(|t| now >= t) && zone.music.is_none() {
        zone.next_track_at = None;
        if let Some(m) = areas.zone_music(zone.zone_music) {
            let kit = m.sounds[phase];
            if kit != 0 {
                start_music_stream(zone, &mut out, &mut kits, &config, kit);
            }
        }
    }

    if let Some(h) = &mut zone.music {
        h.set_volume(
            mixer::amp_to_db(config.category_amp(SoundCategory::Music) * zone.music_kit_vol),
            mixer::glide(),
        );
    }
    if let Some(h) = &zone.music {
        zone.music_watch.feed(h, f64::from(real.delta_secs()));
    }
    let ambience_gain = fade_in_gain(&mut zone.ambience_fade_in, now);
    if let Some(h) = &mut zone.ambience {
        h.set_volume(
            mixer::amp_to_db(
                config.category_amp(SoundCategory::Ambience)
                    * zone.ambience_kit_vol
                    * ambience_gain,
            ),
            mixer::glide(),
        );
    }
}

/// When the next track of the row starts after one ends: after the row's random silence for the
/// phase, or at once with no delay set; never without a row.
fn next_track_time(
    zone: &mut ZoneAudio,
    areas: &AreaSounds,
    row: u32,
    phase: usize,
    now: f64,
    config: &SoundConfig,
) -> Option<f64> {
    if row == 0 {
        return None;
    }
    if config.zone_music_no_delay {
        return Some(now);
    }
    let interval = areas
        .zone_music(row)
        .map(|m| (m.silence_min[phase], m.silence_max[phase]));
    Some(match interval {
        Some((min, max)) => now + f64::from(zone.silence_ms(min, max)) / 1000.0,
        None => now + DEFAULT_SILENCE_SECS,
    })
}

/// Opens a music kit on the slot at full volume, fading out whatever held it; whether it opened.
fn start_music_stream(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    kits: &mut SoundKits,
    config: &SoundConfig,
    kit_id: u32,
) -> bool {
    if out.mixer.is_none() {
        return false;
    }
    let Some((path, kit_vol)) = kits.pick_stream(kit_id) else {
        return false;
    };
    let data = match kits.read(&path).and_then(mixer::stream_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            warn!("zone music: {path}: {e:#}");
            return false;
        }
    };
    let start_amp = config.category_amp(SoundCategory::Music) * kit_vol;
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return false;
    };
    match mixer_ref.play_stream(data.volume(mixer::amp_to_db(start_amp))) {
        Ok(h) => {
            info!("zone music: {path}");
            if let Some(mut outgoing) = zone.music.replace(h) {
                outgoing.stop(mixer::fade(MUSIC_FADE_OUT_MS));
            }
            zone.music_kit_vol = kit_vol;
            out.note_stream("music", kit_id, &path, start_amp);
            true
        }
        Err(e) => {
            warn!("zone music: {path}: {e:#}");
            false
        }
    }
}

/// Fades the bed out over `fade_ms` while the new one starts silent and ramps in over the same;
/// `kit_id` 0 leaves the slot empty.
fn swap_ambience(
    zone: &mut ZoneAudio,
    out: &mut SoundOutput,
    kits: &mut SoundKits,
    config: &SoundConfig,
    kit_id: u32,
    fade_ms: u64,
    now: f64,
) {
    if kit_id == zone.ambience_kit {
        return;
    }
    if let Some(mut h) = zone.ambience.take() {
        h.stop(mixer::fade(fade_ms));
    }
    zone.ambience_kit = kit_id;
    zone.ambience_fade_in = None;
    if kit_id == 0 || out.mixer.is_none() {
        return;
    }
    let Some((path, kit_vol)) = kits.pick_stream(kit_id) else {
        return;
    };
    let data = match kits.read(&path).and_then(mixer::loop_from_bytes) {
        Ok(d) => d,
        Err(e) => {
            warn!("ambience: {path}: {e:#}");
            return;
        }
    };
    let start_amp = if fade_ms > 0 {
        0.0
    } else {
        config.category_amp(SoundCategory::Ambience) * kit_vol
    };
    let Some(mixer_ref) = out.mixer.as_mut() else {
        return;
    };
    match mixer_ref.play_2d(data.volume(mixer::amp_to_db(start_amp))) {
        Ok(h) => {
            info!("ambience: {path}");
            zone.ambience = Some(h);
            zone.ambience_kit_vol = kit_vol;
            if fade_ms > 0 {
                zone.ambience_fade_in = Some(FadeIn {
                    start: now,
                    dur: fade_ms as f64 / 1000.0,
                });
            }
            out.note_stream("ambience", kit_id, &path, start_amp);
        }
        Err(e) => warn!("ambience: {path}: {e:#}"),
    }
}

/// The slots' live streams, counted against the voice ceiling afresh every frame.
pub(crate) fn report_stream_voices(
    zone: NonSend<'_, ZoneAudio>,
    mut out: NonSendMut<'_, SoundOutput>,
) {
    let live = |s: kira::sound::PlaybackState| s != kira::sound::PlaybackState::Stopped;
    out.zone_streams = usize::from(zone.music.as_ref().is_some_and(|h| live(h.state())))
        + usize::from(zone.ambience.as_ref().is_some_and(|h| live(h.state())));
}
