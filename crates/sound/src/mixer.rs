//! The mixer seam: kira behind the narrow surface the client used FMOD through — play and stop,
//! per-channel volume and pitch, the listener and source positions, streamed music. Everything
//! above it computes the client's own parameters; spatial attenuation is ours, so kira pans only.
//!
//! kira's listener is X-right, Y-up, its ears at ±X of its orientation: Bevy's camera space, so
//! transforms go in unchanged.

mod stream_watch;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use bevy::log::{error, info, warn};
use bevy::math::{Quat, Vec3};
use kira::effect::reverb::{ReverbBuilder, ReverbHandle};
use kira::effect::volume_control::{VolumeControlBuilder, VolumeControlHandle};
use kira::listener::ListenerHandle;
use kira::sound::FromFileError;
use kira::sound::streaming::StreamingSoundData;
use kira::track::{SendTrackBuilder, SendTrackHandle, SpatialTrackBuilder};
use kira::{AudioManager, AudioManagerSettings, Decibels, Mix, Tween};

pub use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
pub use kira::sound::streaming::StreamingSoundHandle;
pub use kira::track::SpatialTrackHandle;
pub use stream_watch::StreamWatch;

use crate::meter::{self, LevelReading, MixLevel};
use crate::output::{self, Event, Output, OutputBackend, OutputSettings, Window};
use crate::tables::SoundProvider;
use crate::{limiter, mix_tap};

/// A zero-length change: a true step, such as a reverb preset switch or an initial value.
pub fn snap() -> Tween {
    Tween {
        duration: std::time::Duration::ZERO,
        ..Default::default()
    }
}

/// The per-frame volume feed's ramp. kira applies a volume as one gain per 128-frame block, so a
/// stepped per-frame gain is a click whose loudness scales with the frame hitch; each frame ramps
/// to its new value instead.
pub fn glide() -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(GLIDE_MS),
        ..Default::default()
    }
}

/// Just under a 60 fps frame.
const GLIDE_MS: u64 = 15;

/// The fade on a stop that may cut a sound at full amplitude.
pub fn declick() -> Tween {
    glide()
}

/// A linear fade over `ms`, as the client's constant per-tick decrement is.
pub fn fade(ms: u64) -> Tween {
    Tween {
        duration: std::time::Duration::from_millis(ms),
        ..Default::default()
    }
}

/// Linear amplitude to decibels, with kira's −60 dB floor at or below 10⁻³, already under one
/// step of the 0..255 volume the client fed FMOD.
pub fn amp_to_db(amp: f32) -> Decibels {
    if amp <= 1e-3 {
        Decibels::SILENCE
    } else {
        Decibels(20.0 * amp.log10())
    }
}

/// How the mixer is built.
#[derive(Clone, Debug)]
pub struct MixerSettings {
    pub output: Output,
    /// Record the final mix, before the output gate, to this WAV.
    pub mix_tap: Option<std::path::PathBuf>,
}

/// The one open output and its listener; the only place kira's manager is touched. There is no
/// master filter: the client applied none beyond FMOD's reverb.
pub struct Mixer {
    manager: AudioManager<OutputBackend>,
    listener: ListenerHandle,
    health: MixHealth,
    window: Window,
    /// The zone reverb's wet-only send; every 3-D track that takes reverb routes into it, and its
    /// volume is the zone's wet level.
    reverb_send: SendTrackHandle,
    reverb: ReverbHandle,
    level: Arc<MixLevel>,
    limiter_on: Arc<AtomicBool>,
    /// First in the main chain, ahead of the limiter.
    master: VolumeControlHandle,
    /// Last in the main chain, after every tap: unity or silence, never a level.
    output: VolumeControlHandle,
    audio_pos: Option<Arc<AtomicU64>>,
    sample_rate: Option<u32>,
}

/// How many 3-D voices may live at once: every positional play takes a spatial track of its own.
/// kira's default of 128 is a number a fight reaches; the game's own caps decide what plays, and
/// a refusal past this one is counted.
const SPATIAL_VOICE_CAPACITY: usize = 512;

impl Mixer {
    /// Fails cleanly without a device; the caller runs silent.
    pub fn new(settings: &MixerSettings) -> Result<Self> {
        let sample_rate = output::probe_sample_rate(settings.output);
        let level = Arc::new(MixLevel::default());
        let limiter_on = Arc::new(AtomicBool::new(true));
        let chain = main_track(
            &level,
            &limiter_on,
            sample_rate,
            settings.mix_tap.as_deref(),
        );
        let manager_settings = AudioManagerSettings::<OutputBackend> {
            backend_settings: OutputSettings {
                output: settings.output,
                ..OutputSettings::default()
            },
            main_track_builder: chain.builder,
            capacities: kira::Capacities {
                sub_track_capacity: SPATIAL_VOICE_CAPACITY,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut manager = AudioManager::<OutputBackend>::new(manager_settings)
            .map_err(|e| anyhow::anyhow!("audio device init: {e:#}"))?;
        let sample_rate = Some(manager.backend_mut().sample_rate());
        let mut send_builder = SendTrackBuilder::new().volume(Decibels::SILENCE);
        let reverb = send_builder.add_effect(
            ReverbBuilder::new()
                .mix(Mix::WET)
                .feedback(0.5)
                .damping(0.5),
        );
        let reverb_send = manager
            .add_send_track(send_builder)
            .map_err(|e| anyhow::anyhow!("reverb send alloc: {e}"))?;
        let listener = manager
            .add_listener(mint_vec(Vec3::ZERO), mint_quat(Quat::IDENTITY))
            .map_err(|e| anyhow::anyhow!("listener alloc: {e}"))?;
        Ok(Self {
            manager,
            listener,
            health: MixHealth::default(),
            window: Window::default(),
            reverb_send,
            reverb,
            level,
            limiter_on,
            master: chain.master,
            output: chain.output,
            audio_pos: chain.audio_pos,
            sample_rate,
        })
    }

    /// The mix tap's frame clock, when one records.
    pub fn audio_pos(&self) -> Option<Arc<AtomicU64>> {
        self.audio_pos.clone()
    }

    /// A zone reverb preset, applied at once; `None` silences the send.
    pub fn set_reverb(&mut self, preset: Option<&SoundProvider>) {
        let Some(p) = preset else {
            self.reverb_send.set_volume(Decibels::SILENCE, snap());
            return;
        };
        let (feedback, damping, wet) = freeverb_projection(p);
        self.reverb.set_feedback(feedback, snap());
        self.reverb.set_damping(damping, snap());
        self.reverb_send.set_volume(wet, snap());
    }

    pub fn set_listener(&mut self, pos: Vec3, rot: Quat) {
        self.listener.set_position(mint_vec(pos), snap());
        self.listener.set_orientation(mint_quat(rot), snap());
    }

    /// The whole mix's gain, as linear amplitude.
    pub fn set_master(&mut self, amp: f32) {
        self.master.set_volume(amp_to_db(amp), glide());
    }

    /// Opens or shuts the output after every tap, so recordings keep the mix while the speakers
    /// are silent.
    pub fn set_output_gate(&mut self, open: bool) {
        let db = if open {
            Decibels::IDENTITY
        } else {
            Decibels::SILENCE
        };
        self.output.set_volume(db, glide());
    }

    pub fn set_limiter(&mut self, on: bool) {
        self.limiter_on.store(on, Ordering::Relaxed);
    }

    pub fn take_level(&mut self) -> LevelReading {
        self.level.take()
    }

    pub fn sample_rate(&self) -> Option<u32> {
        self.sample_rate
    }

    /// A decoded sound on the main track: the 2-D path.
    pub fn play_2d(&mut self, data: StaticSoundData) -> Result<StaticSoundHandle> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play 2d: {e}"))
    }

    /// A decoded sound on a spatial track of its own at `pos`, attenuation off. `reverb_send` is
    /// whether the kit takes the zone's reverb at all. The track outlives its handle until its
    /// sound finishes, so a fade-stop and a drop in one breath still fade.
    pub fn play_3d(
        &mut self,
        data: StaticSoundData,
        pos: Vec3,
        reverb_send: bool,
    ) -> Result<(SpatialTrackHandle, StaticSoundHandle)> {
        let mut builder = SpatialTrackBuilder::new()
            .attenuation_function(None)
            .sound_capacity(1);
        if reverb_send {
            builder = builder.with_send(&self.reverb_send, Decibels(0.0));
        }
        let mut track = match self.manager.add_spatial_sub_track(
            self.listener.id(),
            mint_vec(pos),
            builder.persist_until_sounds_finish(true),
        ) {
            Ok(t) => t,
            Err(e) => {
                self.health.voices_refused += 1;
                return Err(anyhow::anyhow!(
                    "spatial track alloc ({SPATIAL_VOICE_CAPACITY}-voice ceiling): {e}"
                ));
            }
        };
        let handle = track
            .play(data)
            .map_err(|e| anyhow::anyhow!("play 3d: {e}"))?;
        Ok((track, handle))
    }

    /// A long sound decoded as it plays, on the main track.
    pub fn play_stream(
        &mut self,
        data: StreamingSoundData<FromFileError>,
    ) -> Result<StreamingSoundHandle<FromFileError>> {
        self.manager
            .play(data)
            .map_err(|e| anyhow::anyhow!("play stream: {e}"))
    }

    /// Offline, renders and returns the next `frames` stereo frames; on a device, nothing.
    pub fn render(&mut self, frames: usize) -> &[f32] {
        self.manager.backend_mut().render(frames)
    }
}

/// How many voices past the ceiling went unheard, and how the output is keeping up.
#[derive(Default, Clone, Copy, Debug)]
pub struct MixHealth {
    pub load: f32,
    pub peak_load: f32,
    /// Device cycles the ring could not fill: each went out as silence.
    pub overruns: u64,
    pub stream_errors: u64,
    pub voices_refused: u64,
}

impl Mixer {
    /// Services the output: device notices, stream rebuilds, the meters. Call every frame.
    pub fn poll_health(&mut self) -> MixHealth {
        let backend = self.manager.backend_mut();
        let (window, events) = backend.service();
        self.sample_rate = Some(backend.sample_rate());
        self.health.stream_errors = backend.stream_errors();
        for event in events {
            match event {
                Event::Opened {
                    device,
                    sample_rate,
                    buffer_frames,
                    mix_ahead_ms,
                    realtime_latency_frames,
                } => info!(
                    "audio: {device}: {sample_rate} Hz, IO buffer {buffer_frames} frames \
                     (~{:.1} ms), mix-ahead {mix_ahead_ms} ms, device latency ~{:.1} ms",
                    f64::from(buffer_frames) / f64::from(sample_rate) * 1000.0,
                    f64::from(realtime_latency_frames) / f64::from(sample_rate) * 1000.0,
                ),
                Event::Lost(why) => warn!("audio: output stream dropped: {why}; reopening"),
                Event::OpenFailed(what) => warn!("audio: output device refused: {what}; retrying"),
                Event::Dead(what) => error!("audio: output is gone for good: {what}"),
            }
        }
        if window.render_chunks > 0 && window.chunk_ms > 0.0 {
            self.health.load = (window.render_wall_max_ms / window.chunk_ms) as f32;
            self.health.peak_load = self.health.peak_load.max(self.health.load);
        }
        self.health.overruns += window.underruns;
        self.window.merge(window);
        self.health
    }

    pub fn take_health_peak(&mut self) -> f32 {
        std::mem::take(&mut self.health.peak_load)
    }

    pub(crate) fn take_output_window(&mut self) -> Window {
        std::mem::take(&mut self.window)
    }
}

/// The main track's chain in signal order: master, meter, limiter, tap, output gate. kira
/// applies a track's own volume after its effects, so the master is an effect to stay ahead of
/// the limiter.
fn main_track(
    level: &Arc<MixLevel>,
    limiter_on: &Arc<AtomicBool>,
    sample_rate: Option<u32>,
    mix_tap: Option<&Path>,
) -> MainChain {
    let mut main = kira::track::MainTrackBuilder::new();
    let master = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    meter::install(&mut main, level);
    limiter::install(&mut main, level, limiter_on);
    let audio_pos = match (mix_tap, sample_rate) {
        (Some(path), Some(rate)) => mix_tap::install_at(&mut main, path, rate),
        (Some(_), None) => {
            warn!("mix tap: the device's sample rate is unknown, not recording");
            None
        }
        _ => None,
    };
    let output = main.add_effect(VolumeControlBuilder::new(Decibels::IDENTITY));
    MainChain {
        builder: main,
        master,
        output,
        audio_pos,
    }
}

struct MainChain {
    builder: kira::track::MainTrackBuilder,
    master: VolumeControlHandle,
    output: VolumeControlHandle,
    audio_pos: Option<Arc<AtomicU64>>,
}

/// Moves a live spatial track's emitter.
pub fn set_track_position(track: &mut SpatialTrackHandle, pos: Vec3) {
    track.set_position(mint_vec(pos), snap());
}

fn mint_vec(v: Vec3) -> mint::Vector3<f32> {
    mint::Vector3 {
        x: v.x,
        y: v.y,
        z: v.z,
    }
}

fn mint_quat(q: Quat) -> mint::Quaternion<f32> {
    mint::Quaternion {
        v: mint::Vector3 {
            x: q.x,
            y: q.y,
            z: q.z,
        },
        s: q.w,
    }
}

/// An EAX preset as Freeverb `(feedback, damping, wet)`: feedback from the decay time through
/// Freeverb's RT60 at its mean comb delay, capped short of runaway; damping from the high
/// frequency decay ratio and the room's high cut; wet from room plus reverb, at most +6 dB.
fn freeverb_projection(p: &SoundProvider) -> (f64, f64, Decibels) {
    let feedback = if p.decay_time > 0.0 {
        10f64.powf(-0.108 / f64::from(p.decay_time)).min(0.98)
    } else {
        0.0
    };
    let damping = ((1.0 - f64::from(p.decay_hf_ratio) / 2.0).max(0.0) * 0.7
        + f64::from(-p.room_hf) / 10_000.0 * 0.3)
        .clamp(0.0, 1.0);
    let wet_db = ((p.room + p.reverb) as f32 / 100.0).min(6.0);
    let wet = if wet_db <= Decibels::SILENCE.0 {
        Decibels::SILENCE
    } else {
        Decibels(wet_db)
    };
    (feedback, damping, wet)
}

/// A short effect, decoded whole: WAV including IMA ADPCM, or MP3.
pub fn sfx_from_bytes(bytes: Vec<u8>) -> Result<StaticSoundData> {
    StaticSoundData::from_cursor(std::io::Cursor::new(bytes)).context("decoding sfx")
}

/// A looping bed, decoded whole: kira's stream decoder misreads these 22050 Hz PCM beds at twice
/// their length, so a streamed loop runs past the end and dies.
pub fn loop_from_bytes(bytes: Vec<u8>) -> Result<StaticSoundData> {
    Ok(sfx_from_bytes(bytes)?.loop_region(..))
}

/// Compressed audio decoded as it plays, through a source that raises its decode thread's quality
/// of service.
pub fn stream_from_bytes(bytes: Vec<u8>) -> Result<StreamingSoundData<FromFileError>> {
    StreamingSoundData::from_media_source(PromotingSource(std::io::Cursor::new(bytes)))
        .context("opening stream")
}

/// kira decodes each stream on a bare thread of its own at the default quality of service, under
/// everything a busy frame runs; starved, the stream zero-fills whole blocks. The decoder's reads
/// are the only code of ours on that thread, so the first one raises it.
struct PromotingSource(std::io::Cursor<Vec<u8>>);

fn promote_decode_thread() {
    std::thread_local! {
        static PROMOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    PROMOTED.with(|p| {
        if !p.get() {
            output::promote_current_thread();
            p.set(true);
        }
    });
}

impl std::io::Read for PromotingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        promote_decode_thread();
        self.0.read(buf)
    }
}

impl std::io::Seek for PromotingSource {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        promote_decode_thread();
        self.0.seek(pos)
    }
}

impl symphonia::core::io::MediaSource for PromotingSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.get_ref().len() as u64)
    }
}
