mod device;
mod offline;
#[cfg(target_os = "macos")]
#[path = "output/coreaudio.rs"]
mod platform;
#[cfg(not(target_os = "macos"))]
#[path = "output/cpal.rs"]
mod platform;

use anyhow::Result;
use kira::backend::{Backend, Renderer};

pub(crate) use device::{Event, Window};
pub(crate) use platform::promote_current_thread;

pub(crate) const DEVICE_BUFFER_FRAMES: u32 = 512;
/// How far ahead of the device the mix runs: the larger of the client's two defaults for the same
/// dial, the stall the output can absorb without a gap.
pub(crate) const MIX_AHEAD_MS: u32 = 100;
/// The rate to render an offline mix at.
pub const OFFLINE_SAMPLE_RATE: u32 = 48_000;

/// Where the mix goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// The system's default output device.
    Device,
    /// Nowhere: the app renders it one frame at a time, at this rate.
    Offline { sample_rate: u32 },
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct OutputSettings {
    pub(crate) output: Output,
    pub(crate) device_buffer_frames: u32,
    pub(crate) mix_ahead_ms: u32,
}

impl Default for OutputSettings {
    fn default() -> Self {
        Self {
            output: Output::Device,
            device_buffer_frames: DEVICE_BUFFER_FRAMES,
            mix_ahead_ms: MIX_AHEAD_MS,
        }
    }
}

#[allow(clippy::large_enum_variant, reason = "the one backend, never moved")]
pub(crate) enum OutputBackend {
    Device(device::DeviceOutput),
    Offline(offline::OfflineOutput),
}

impl Backend for OutputBackend {
    type Settings = OutputSettings;
    type Error = anyhow::Error;

    fn setup(settings: OutputSettings, internal_buffer_size: usize) -> Result<(Self, u32)> {
        match settings.output {
            Output::Device => {
                let (out, rate) = device::DeviceOutput::setup(settings)?;
                Ok((Self::Device(out), rate))
            }
            Output::Offline { sample_rate } => Ok((
                Self::Offline(offline::OfflineOutput::new(
                    sample_rate,
                    internal_buffer_size,
                )),
                sample_rate,
            )),
        }
    }

    fn start(&mut self, renderer: Renderer) -> Result<()> {
        match self {
            Self::Device(out) => out.start(renderer),
            Self::Offline(out) => {
                out.start(renderer);
                Ok(())
            }
        }
    }
}

impl OutputBackend {
    pub(crate) fn sample_rate(&self) -> u32 {
        match self {
            Self::Device(out) => out.sample_rate(),
            Self::Offline(out) => out.sample_rate(),
        }
    }

    pub(crate) fn stream_errors(&self) -> u64 {
        match self {
            Self::Device(out) => out.stream_errors(),
            Self::Offline(_) => 0,
        }
    }

    pub(crate) fn service(&mut self) -> (Window, Vec<Event>) {
        match self {
            Self::Device(out) => out.service(),
            Self::Offline(_) => (Window::default(), Vec::new()),
        }
    }

    pub(crate) fn render(&mut self, frames: usize) -> &[f32] {
        match self {
            Self::Device(_) => &[],
            Self::Offline(out) => out.render(frames),
        }
    }
}

pub(crate) fn probe_sample_rate(output: Output) -> Option<u32> {
    match output {
        Output::Device => platform::default_output().ok().map(|d| d.sample_rate),
        Output::Offline { sample_rate } => Some(sample_rate),
    }
}
