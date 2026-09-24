use std::path::PathBuf;

use bevy::prelude::*;
use world::{Install, WorldCamera};

use crate::config::SoundConfig;
use crate::kit::{self, ActiveChannel, Played, SoundKits};
use crate::log::PlayLog;
use crate::mixer::{Mixer, MixerSettings};
use crate::output::Output;
use crate::tables::KitCatalog;

/// The output and every live kit channel.
pub struct SoundOutput {
    /// `None` without a device: every sound is then refused at the mixer.
    pub mixer: Option<Mixer>,
    pub(crate) channels: Vec<ActiveChannel>,
    pub(crate) zone_streams: usize,
    pub voices_stolen: u64,
    pub voices_denied: u64,
    pub copies_dropped: u64,
    pub(crate) log: Option<PlayLog>,
    pub(crate) play_time: f64,
    pub(crate) offline: Option<crate::health::OfflineClock>,
}

impl SoundOutput {
    /// Everything the output mixes: kit channels and held streams.
    pub fn live_voices(&self) -> usize {
        self.channels.len() + self.zone_streams
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub(crate) fn note_play(
        &mut self,
        played: &Played,
        category: &str,
        spatial: &str,
        pos: Option<Vec3>,
    ) {
        if let Some(log) = self.log.as_mut() {
            log.play(self.play_time, played, category, spatial, pos);
        }
    }

    pub(crate) fn note_stream(&mut self, slot: &str, kit: u32, path: &str, amp: f32) {
        if let Some(log) = self.log.as_mut() {
            log.stream(self.play_time, slot, kit, path, amp);
        }
    }
}

/// Where the 3-D listener is this frame, in Bevy space.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct AudioListener {
    pub pos: Vec3,
    pub rot: Quat,
}

/// The character the listener sits on, when there is one: its head and the yaw it faces. The app
/// writes it; without one, or with the listener set to the camera, the camera hears.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct ListenerCharacter(pub Option<(Vec3, f32)>);

/// The sound's per-frame work in `Update`, after the world's; order whatever moves the character
/// and the camera before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SoundSystems;

/// Plays the world's sound around the listener. Needs the `world` plugins and [`Install`].
pub struct SoundPlugin {
    pub output: Output,
    /// Record the mix, before the output gate, to a WAV.
    pub mix_tap: Option<PathBuf>,
    /// Offline, write every rendered frame to a WAV.
    pub record: Option<PathBuf>,
    /// Log every play, one JSON line each.
    pub log: Option<PathBuf>,
}

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        let settings = MixerSettings {
            output: self.output,
            mix_tap: self.mix_tap.clone(),
        };
        let mixer = match Mixer::new(&settings) {
            Ok(m) => Some(m),
            Err(e) => {
                warn!("no audio device, running silent: {e:#}");
                None
            }
        };
        let offline = match (self.output, mixer.as_ref()) {
            (Output::Offline { sample_rate }, Some(_)) => Some(crate::health::OfflineClock::new(
                sample_rate,
                self.record.as_deref(),
            )),
            _ => None,
        };
        let log = self.log.as_deref().and_then(PlayLog::create);
        app.insert_non_send_resource(SoundOutput {
            mixer,
            channels: Vec::new(),
            zone_streams: 0,
            voices_stolen: 0,
            voices_denied: 0,
            copies_dropped: 0,
            log,
            play_time: 0.0,
            offline,
        })
        .insert_non_send_resource(crate::zone::ZoneAudio::default())
        .init_resource::<SoundConfig>()
        .init_resource::<AudioListener>()
        .init_resource::<ListenerCharacter>()
        .init_resource::<crate::interior::CurrentInterior>()
        .init_resource::<crate::reverb::AppliedPreset>()
        .init_resource::<crate::emitter_pool::AmbientEmitterPool>()
        .init_resource::<crate::liquid_loop::LiquidLoopState>()
        .add_systems(
            Startup,
            (
                load_kits,
                crate::zone::load_area_sounds,
                load_providers,
                crate::footsteps::load_footsteps,
                crate::liquid_loop::load_water_sounds,
            ),
        )
        .add_systems(PreUpdate, stamp_clock)
        .configure_sets(
            Update,
            SoundSystems
                .after(world::WorldSystems)
                .after(world::unit::UnitSystems)
                .after(world::EventSystems),
        )
        .add_systems(
            Update,
            (
                update_audio_listener,
                apply_master_volume,
                apply_focus_gate,
                crate::interior::resolve_interior,
                crate::zone::zone_audio,
                crate::zone::report_stream_voices,
                crate::reverb::zone_reverb,
                crate::anim_events::route_anim_events,
                crate::emitter_pool::release_on_despawn,
                crate::emitter_pool::pump_emitters,
                crate::footsteps::footstep_sounds,
                crate::water::water_splashes,
                crate::liquid_loop::drive_liquid_loops,
                kit::pump_channels,
                crate::health::service_output,
            )
                .chain()
                .in_set(SoundSystems),
        )
        .add_systems(Last, crate::health::render_offline);
    }
}

fn load_kits(mut commands: Commands<'_, '_>, install: Option<Res<'_, Install>>) {
    let Some(install) = install else { return };
    match KitCatalog::load(&install.0) {
        Ok(catalog) => {
            info!("sound: {} kits", catalog.len());
            commands.insert_resource(SoundKits::new(catalog, install.0.clone()));
        }
        Err(e) => warn!("sound: no kits, so no sound: {e}"),
    }
}

fn load_providers(mut commands: Commands<'_, '_>, install: Option<Res<'_, Install>>) {
    let Some(install) = install else { return };
    match crate::tables::SoundProviders::load(&install.0) {
        Ok(cat) => {
            commands.insert_resource(cat);
        }
        Err(e) => warn!("sound: no reverb presets: {e}"),
    }
}

fn stamp_clock(mut out: NonSendMut<'_, SoundOutput>, time: Res<'_, Time>) {
    out.play_time = time.elapsed_secs_f64();
}

fn update_audio_listener(
    mut listener: ResMut<'_, AudioListener>,
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    character: Res<'_, ListenerCharacter>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
) {
    let pose = match character.0 {
        Some((head, facing)) if config.listener_at_character => {
            Some((head, Quat::from_rotation_y(facing)))
        }
        _ => camera.single().ok().map(|t| (t.translation, t.rotation)),
    };
    let Some((pos, rot)) = pose else { return };
    *listener = AudioListener { pos, rot };
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_listener(pos, rot);
    }
}

fn apply_master_volume(
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    mut last: Local<'_, Option<(bool, f32, bool)>>,
) {
    let cur = (
        config.enabled && !config.muted,
        config.master,
        config.limiter,
    );
    if *last == Some(cur) {
        return;
    }
    *last = Some(cur);
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_master(if cur.0 { cur.1 } else { 0.0 });
        mixer.set_limiter(cur.2);
    }
}

fn apply_focus_gate(
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    windows: Query<'_, '_, &Window, With<bevy::window::PrimaryWindow>>,
    mut last: Local<'_, Option<bool>>,
) {
    let backgrounded = windows.single().is_ok_and(|w| !w.focused);
    let open = config.background_sound || !backgrounded;
    if *last == Some(open) {
        return;
    }
    *last = Some(open);
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_output_gate(open);
    }
}
