//! The device layer everywhere but macOS, over cpal, with the surface `coreaudio.rs` has. cpal
//! splits a cycle's timestamp in two, so the due time is rebuilt on our clock; it has no device
//! notices, so a thread polls the default output; and no workgroups, so [`Workgroup`] is inert.
//! An unsupported target gets cpal's null host, which reports no device.
#![allow(
    unsafe_code,
    reason = "SCHED_FIFO and MMCSS are C APIs with no safe binding for these calls"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};

pub(super) const CHANNELS: u32 = 2;

/// How often the default output is re-read: half a second, kira's own cadence.
const WATCH_EVERY: Duration = Duration::from_millis(500);
/// The most frames one [`Cycle`] carries; a longer host buffer is served in several.
const MAX_CYCLE_FRAMES: usize = 8192;

/// Nanoseconds on a monotonic clock of our own: cpal's instants compare only within one host.
pub(super) fn now_ns() -> u64 {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

#[derive(Clone)]
pub(super) struct Device {
    pub name: String,
    pub sample_rate: u32,
    pub buffer_range: (u32, u32),
    /// cpal reports neither; the report prints 0.
    pub latency_frames: u32,
    pub safety_frames: u32,
    handle: cpal::Device,
    config: cpal::SupportedStreamConfig,
    /// The host's stable id: a display name is neither unique nor fixed.
    id: Option<cpal::DeviceId>,
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device")
            .field("name", &self.name)
            .field("sample_rate", &self.sample_rate)
            .field("buffer_range", &self.buffer_range)
            .finish_non_exhaustive()
    }
}

pub(super) fn default_output() -> Result<Device> {
    // The epoch starts here, on the main thread, never on the audio callback.
    now_ns();
    let device = cpal::default_host()
        .default_output_device()
        .context("no default output device")?;
    describe(device)
}

fn describe(device: cpal::Device) -> Result<Device> {
    let name = device.description().map_or_else(
        |_| "unnamed output device".to_string(),
        |d| d.name().to_string(),
    );
    // The one config the host guarantees it can open.
    let config = device
        .default_output_config()
        .with_context(|| format!("default output config for {name}"))?;
    let sample_rate = config.sample_rate();
    if !(8000..=384_000).contains(&sample_rate) {
        bail!("device {name} reports an absurd sample rate {sample_rate}");
    }
    if config.channels() == 0 {
        bail!("device {name} reports zero output channels");
    }
    let buffer_range = match *config.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => (min.max(1), max.max(1)),
        cpal::SupportedBufferSize::Unknown => (1, u32::MAX),
    };
    Ok(Device {
        name,
        sample_rate,
        buffer_range,
        latency_frames: 0,
        safety_frames: 0,
        id: device.id().ok(),
        handle: device,
        config,
    })
}

pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub output_time_ns: u64,
}

pub(super) struct Stream {
    _stream: cpal::Stream,
    observed_frames: Arc<AtomicU32>,
}

impl Stream {
    /// The cycle size of the last callback: a shared-mode host varies it from wake to wake.
    pub(super) fn buffer_frames(&self) -> u32 {
        self.observed_frames.load(Ordering::Relaxed)
    }

    /// `notices` takes cpal's error callback, which these hosts deliver.
    pub(super) fn open<F>(
        device: &Device,
        buffer_frames: u32,
        notices: &Arc<Notices>,
        on_cycle: F,
    ) -> Result<Self>
    where
        F: FnMut(Cycle<'_>) + Send + 'static,
    {
        let buffer_frames = buffer_frames.clamp(device.buffer_range.0, device.buffer_range.1);
        let mut config = device.config.config();
        config.buffer_size = match device.config.buffer_size() {
            cpal::SupportedBufferSize::Range { .. } => cpal::BufferSize::Fixed(buffer_frames),
            cpal::SupportedBufferSize::Unknown => cpal::BufferSize::Default,
        };
        let observed_frames = Arc::new(AtomicU32::new(buffer_frames));
        let errors = Arc::clone(notices);
        let reported = AtomicBool::new(false);
        // Only a vanished device or an invalidated stream is fatal; a routine xrun keeps the
        // stream, and rebuilding on one would be the loudest answer to the quietest problem.
        let on_error = move |e: cpal::StreamError| {
            if matches!(
                e,
                cpal::StreamError::DeviceNotAvailable | cpal::StreamError::StreamInvalidated
            ) {
                errors.device_died.store(true, Ordering::Release);
                bevy::log::warn!("audio: output stream lost ({e})");
            } else if !reported.swap(true, Ordering::Relaxed) {
                bevy::log::warn!("audio: output stream reported {e} (stream kept)");
            }
        };
        let ctx = Callback {
            on_cycle,
            channels: usize::from(device.config.channels()),
            sample_rate: device.sample_rate,
            observed: Arc::clone(&observed_frames),
            scratch: vec![0.0; MAX_CYCLE_FRAMES * CHANNELS as usize],
        };
        let stream = match device.config.sample_format() {
            cpal::SampleFormat::F32 => build::<f32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::F64 => build::<f64, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I8 => build::<i8, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I16 => build::<i16, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I24 => build::<cpal::I24, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I32 => build::<i32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::I64 => build::<i64, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U8 => build::<u8, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U16 => build::<u16, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U32 => build::<u32, _>(device, &config, ctx, on_error),
            cpal::SampleFormat::U64 => build::<u64, _>(device, &config, ctx, on_error),
            other => bail!("device {} wants sample format {other}", device.name),
        }?;
        stream.play().context("starting the output stream")?;
        Ok(Self {
            _stream: stream,
            observed_frames,
        })
    }
}

struct Callback<F> {
    on_cycle: F,
    channels: usize,
    sample_rate: u32,
    observed: Arc<AtomicU32>,
    scratch: Vec<f32>,
}

fn build<T, F>(
    device: &Device,
    config: &cpal::StreamConfig,
    mut ctx: Callback<F>,
    on_error: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
    F: FnMut(Cycle<'_>) + Send + 'static,
{
    device
        .handle
        .build_output_stream::<T, _, _>(
            config,
            move |data, info| ctx.run(data, info),
            on_error,
            None,
        )
        .with_context(|| format!("opening an output stream on {}", device.name))
}

impl<F: FnMut(Cycle<'_>) + Send + 'static> Callback<F> {
    fn run<T>(&mut self, data: &mut [T], info: &cpal::OutputCallbackInfo)
    where
        T: SizedSample + FromSample<f32>,
    {
        let frames = data.len() / self.channels;
        if frames == 0 {
            return;
        }
        self.observed.store(frames as u32, Ordering::Relaxed);
        let ts = info.timestamp();
        let due = match ts.playback.duration_since(&ts.callback) {
            Some(ahead) => now_ns() + ahead.as_nanos() as u64,
            None => 0,
        };
        let per_cycle = self.scratch.len() / CHANNELS as usize;
        let mut done = 0;
        while done < frames {
            let take = (frames - done).min(per_cycle);
            let stereo = &mut self.scratch[..take * CHANNELS as usize];
            (self.on_cycle)(Cycle {
                buffer: stereo,
                output_time_ns: if due == 0 {
                    0
                } else {
                    due + (done as u64 * 1_000_000_000) / u64::from(self.sample_rate.max(1))
                },
            });
            spread(stereo, &mut data[done * self.channels..], self.channels);
            done += take;
        }
    }
}

/// Stereo over the device's channels: mono averages the pair, and anything wider carries left
/// and right on its first two and silence on the rest, never an invented upmix.
fn spread<T: SizedSample + FromSample<f32>>(stereo: &[f32], out: &mut [T], channels: usize) {
    match channels {
        1 => {
            for (frame, slot) in stereo.as_chunks::<2>().0.iter().zip(out.iter_mut()) {
                *slot = T::from_sample(f32::midpoint(frame[0], frame[1]));
            }
        }
        2 => {
            for (sample, slot) in stereo.iter().zip(out.iter_mut()) {
                *slot = T::from_sample(*sample);
            }
        }
        n => {
            for (frame, slot) in stereo
                .as_chunks::<2>()
                .0
                .iter()
                .zip(out.chunks_exact_mut(n))
            {
                slot[0] = T::from_sample(frame[0]);
                slot[1] = T::from_sample(frame[1]);
                for quiet in &mut slot[2..] {
                    *quiet = T::from_sample(0.0f32);
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Notices {
    pub default_changed: AtomicBool,
    pub device_died: AtomicBool,
    pub rate_changed: AtomicBool,
    /// No host here reports an overload; it stays zero.
    pub overloads: AtomicU64,
    pub last_overload_ns: AtomicU64,
}

/// A one-shot watch on the default output: it stops at its first notice, because the answer is
/// a new stream, which arms a new watch.
pub(super) struct Listeners {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Listeners {
    pub(super) fn arm(device: &Device, notices: Arc<Notices>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let watching = Arc::clone(&stop);
        let (was_id, was_name, was_rate) =
            (device.id.clone(), device.name.clone(), device.sample_rate);
        let thread = std::thread::Builder::new()
            .name("audio-devwatch".into())
            .spawn(move || {
                while !watching.load(Ordering::Acquire) {
                    std::thread::park_timeout(WATCH_EVERY);
                    if watching.load(Ordering::Acquire) {
                        return;
                    }
                    let Some(now) = cpal::default_host().default_output_device() else {
                        notices.device_died.store(true, Ordering::Release);
                        return;
                    };
                    // An unreadable device is a transient, not a change.
                    let changed = match (&was_id, now.id()) {
                        (Some(was), Ok(is_now)) => Some(*was != is_now),
                        _ => now.description().ok().map(|d| d.name() != was_name),
                    };
                    if changed == Some(true) {
                        notices.default_changed.store(true, Ordering::Release);
                        return;
                    }
                    if now
                        .default_output_config()
                        .is_ok_and(|c| c.sample_rate() != was_rate)
                    {
                        notices.rate_changed.store(true, Ordering::Release);
                        return;
                    }
                }
            })
            .ok();
        if thread.is_none() {
            bevy::log::warn!("audio: no device watch; device changes will not be followed");
        }
        Self { stop, thread }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

pub(super) struct Workgroup;

impl Workgroup {
    pub(super) fn of_device(_device: &Device) -> Option<Self> {
        None
    }
}

pub(super) struct Joined;

impl Joined {
    pub(super) fn join(_group: Workgroup) -> Result<Self> {
        bail!("no audio workgroups on this platform")
    }
}

pub(super) struct Realtime {
    #[cfg(windows)]
    _task: mmcss::Task,
}

/// `SCHED_FIFO` at 10, inside the ceiling a desktop hands its audio clients, on unix; MMCSS
/// "Pro Audio" on Windows. Either may be refused, which the ring absorbs.
pub(super) fn set_realtime(_period_ns: u64) -> Result<Realtime> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // SAFETY: a POSIX call on the calling thread with a zeroed, correctly typed parameter.
        let rc = unsafe {
            let mut param: libc::sched_param = std::mem::zeroed();
            param.sched_priority = 10;
            libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &raw const param)
        };
        if rc != 0 {
            let why = std::io::Error::from_raw_os_error(rc);
            bail!("pthread_setschedparam(SCHED_FIFO, 10) failed: {why}");
        }
        Ok(Realtime {})
    }
    #[cfg(windows)]
    {
        Ok(Realtime {
            _task: mmcss::Task::join("Pro Audio")?,
        })
    }
    #[cfg(not(any(all(unix, not(target_os = "macos")), windows)))]
    {
        bail!("no realtime thread policy on this platform")
    }
}

/// Quality-of-service classes are macOS's; nothing to raise here.
pub(crate) fn promote_current_thread() {}

#[cfg(windows)]
mod mmcss {
    use anyhow::{Result, bail};

    const PRIORITY_CRITICAL: i32 = 2;

    #[link(name = "avrt")]
    unsafe extern "system" {
        fn AvSetMmThreadCharacteristicsW(
            task_name: *const u16,
            task_index: *mut u32,
        ) -> *mut core::ffi::c_void;
        fn AvSetMmThreadPriority(handle: *mut core::ffi::c_void, priority: i32) -> i32;
        fn AvRevertMmThreadCharacteristics(handle: *mut core::ffi::c_void) -> i32;
    }

    /// The calling thread's MMCSS membership, reverted on drop on the same thread.
    pub(super) struct Task(*mut core::ffi::c_void);

    // SAFETY: only the creating thread touches the handle; this keeps `Realtime`'s auto traits
    // the same on every target.
    unsafe impl Send for Task {}

    impl Task {
        pub(super) fn join(name: &str) -> Result<Self> {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let mut index = 0u32;
            // SAFETY: a NUL-terminated wide string and an out-parameter.
            let handle = unsafe { AvSetMmThreadCharacteristicsW(wide.as_ptr(), &raw mut index) };
            if handle.is_null() {
                bail!("AvSetMmThreadCharacteristicsW(\"{name}\") was refused");
            }
            // SAFETY: the live registration just returned.
            if unsafe { AvSetMmThreadPriority(handle, PRIORITY_CRITICAL) } == 0 {
                bevy::log::warn!("audio: MMCSS took the task but refused the critical priority");
            }
            Ok(Self(handle))
        }
    }

    impl Drop for Task {
        fn drop(&mut self) {
            // SAFETY: mirrors the registration, once.
            unsafe { AvRevertMmThreadCharacteristics(self.0) };
        }
    }
}
