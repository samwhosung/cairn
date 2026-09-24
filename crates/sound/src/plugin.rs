use std::path::PathBuf;

use bevy::prelude::*;
use world::{Install, WorldCamera};

use crate::config::SoundConfig;
use crate::kit::{self, ActiveChannel, Played, SoundKits};
use crate::log::PlayLog;
use crate::mixer::{Mixer, MixerSettings};
use crate::output::Output;
use crate::tables::KitCatalog;

/// The output and every live kit channel. Not `Send`: a device stream is not on every platform.
pub struct SoundOutput {
    /// `None` without a device: every sound is then refused at the mixer.
    pub mixer: Option<Mixer>,
    pub(crate) channels: Vec<ActiveChannel>,
    /// The zone's live music and ambience streams, which count against the voice ceiling too.
    pub(crate) zone_streams: usize,
    pub voices_stolen: u64,
    pub voices_denied: u64,
    pub copies_dropped: u64,
    pub(crate) log: Option<PlayLog>,
    /// The game time plays are logged at.
    pub(crate) clock: f64,
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
            log.play(self.clock, played, category, spatial, pos);
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
            clock: 0.0,
            offline,
        })
        .init_resource::<SoundConfig>()
        .init_resource::<AudioListener>()
        .init_resource::<ListenerCharacter>()
        .add_systems(Startup, (load_kits,))
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
                kit::pump_channels,
                crate::health::poll_mix_health,
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

fn stamp_clock(mut out: NonSendMut<'_, SoundOutput>, time: Res<'_, Time>) {
    out.clock = time.elapsed_secs_f64();
}

/// The listener on the character's head facing where the body faces, so a zoom or an orbit of the
/// camera never moves a sound; on the camera when there is no character to sit on.
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

/// Only on a change: a slider dragged every frame would restart the glide forever.
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

/// The client goes quiet while its window is in the background. A missing window keeps the gate
/// open: not knowing whether anyone looks must not be heard as silence.
fn apply_focus_gate(
    mut out: NonSendMut<'_, SoundOutput>,
    config: Res<'_, SoundConfig>,
    windows: Query<'_, '_, &Window, With<bevy::window::PrimaryWindow>>,
    mut last: Local<'_, Option<bool>>,
) {
    let open = config.background_sound || windows.single().map_or(true, |w| w.focused);
    if *last == Some(open) {
        return;
    }
    *last = Some(open);
    if let Some(mixer) = out.mixer.as_mut() {
        mixer.set_output_gate(open);
    }
}
