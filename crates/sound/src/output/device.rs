//! The device output: the render thread, the ring between it and the IO callback, the stream's
//! rebuild when the device changes, and the meters that say whether each cycle was met.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use bevy::log::{debug, warn};
use kira::backend::Renderer;
use rtrb::{Consumer, Producer, RingBuffer};

use super::OutputSettings;
use super::platform::{self, Cycle, Device, Joined, Listeners, Notices, Stream, Workgroup};

/// Frames per render pass: two of kira's 128-frame parameter blocks.
const RENDER_CHUNK_FRAMES: usize = 256;
const REOPEN_EVERY: Duration = Duration::from_secs(1);
/// How long the main thread waits for a dropped stream's callback to hand the ring back.
const HANDBACK_WAIT: Duration = Duration::from_millis(250);

/// One `service` window, in the report's units.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Window {
    pub(crate) cycles: u64,
    /// The smallest time left before the device needed a cycle's buffer, ms; negative is late.
    pub(crate) lead_min_ms: Option<f64>,
    pub(crate) io_wall_max_ms: f64,
    pub(crate) gap_max_ms: f64,
    /// Cycles the ring could not fill, and the silence that went out for them.
    pub(crate) underruns: u64,
    pub(crate) underrun_ms: f64,
    pub(crate) ring_min_ms: Option<f64>,
    pub(crate) render_wall_max_ms: f64,
    pub(crate) render_chunks: u64,
    /// The device's own overload notices; macOS only.
    pub(crate) overloads: u64,
    pub(crate) last_overload_ago_ms: Option<f64>,
    pub(crate) cycle_ms: f64,
    pub(crate) chunk_ms: f64,
}

impl Window {
    pub(crate) fn merge(&mut self, later: Window) {
        self.cycles += later.cycles;
        self.lead_min_ms = min_of(self.lead_min_ms, later.lead_min_ms);
        self.io_wall_max_ms = self.io_wall_max_ms.max(later.io_wall_max_ms);
        self.gap_max_ms = self.gap_max_ms.max(later.gap_max_ms);
        self.underruns += later.underruns;
        self.underrun_ms += later.underrun_ms;
        self.ring_min_ms = min_of(self.ring_min_ms, later.ring_min_ms);
        self.render_wall_max_ms = self.render_wall_max_ms.max(later.render_wall_max_ms);
        self.render_chunks += later.render_chunks;
        self.overloads += later.overloads;
        if later.last_overload_ago_ms.is_some() {
            self.last_overload_ago_ms = later.last_overload_ago_ms;
        }
        self.cycle_ms = later.cycle_ms;
        self.chunk_ms = later.chunk_ms;
    }
}

fn min_of(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

#[derive(Debug)]
pub(crate) enum Event {
    Opened {
        device: String,
        sample_rate: u32,
        buffer_frames: u32,
        mix_ahead_ms: u32,
        realtime_latency_frames: u32,
    },
    Lost(&'static str),
    OpenFailed(String),
    /// The ring never came back from a dropped stream: nothing can reopen.
    Dead(String),
}

#[derive(Default)]
struct Meters {
    cycles: AtomicU64,
    lead_min_ns: AtomicI64,
    io_wall_max_ns: AtomicU64,
    gap_max_ns: AtomicU64,
    underruns: AtomicU64,
    underrun_frames: AtomicU64,
    ring_min_frames: AtomicUsize,
    render_wall_max_ns: AtomicU64,
    render_chunks: AtomicU64,
}

impl Meters {
    fn reset_extrema(&self) {
        self.lead_min_ns.store(i64::MAX, Ordering::Relaxed);
        self.ring_min_frames.store(usize::MAX, Ordering::Relaxed);
    }
}

/// The render thread only ever `try_lock`s the mutex, so the audio path never blocks on it.
#[derive(Default)]
struct Control {
    stop: AtomicBool,
    pending_rate: AtomicU32,
    regroup: AtomicBool,
    new_group: Mutex<Option<Workgroup>>,
    render_thread: OnceLock<std::thread::Thread>,
}

#[derive(Default)]
struct Shared {
    meters: Meters,
    control: Control,
    notices: Arc<Notices>,
}

/// Hands `T` back through a one-slot ring when dropped: how the ring consumer inside the IO
/// closure returns when its stream is torn down.
struct Returning<T> {
    inner: Option<T>,
    back: Producer<T>,
}

impl<T> Returning<T> {
    fn new(value: T) -> (Self, Consumer<T>) {
        let (back, receiver) = RingBuffer::new(1);
        (
            Self {
                inner: Some(value),
                back,
            },
            receiver,
        )
    }

    fn get_mut(&mut self) -> Option<&mut T> {
        self.inner.as_mut()
    }
}

impl<T> Drop for Returning<T> {
    fn drop(&mut self) {
        if let Some(value) = self.inner.take() {
            let _ = self.back.push(value);
        }
    }
}

enum Stage {
    Set(Device),
    Running {
        stream: Stream,
        _listeners: Listeners,
        handback: Consumer<Consumer<f32>>,
    },
    Idle {
        consumer: Consumer<f32>,
        since: Instant,
    },
    Dead,
}

pub(crate) struct DeviceOutput {
    settings: OutputSettings,
    shared: Arc<Shared>,
    stage: Stage,
    sample_rate: u32,
    buffer_frames: u32,
    render: Option<std::thread::JoinHandle<()>>,
    pending: Vec<Event>,
    overloads_seen: u64,
    stream_errors: u64,
}

impl DeviceOutput {
    pub(crate) fn setup(settings: OutputSettings) -> Result<(Self, u32)> {
        let device = platform::default_output()?;
        let shared = Arc::new(Shared::default());
        shared.meters.reset_extrema();
        let sample_rate = device.sample_rate;
        Ok((
            Self {
                settings,
                shared,
                stage: Stage::Set(device),
                sample_rate,
                buffer_frames: settings.device_buffer_frames,
                render: None,
                pending: Vec::new(),
                overloads_seen: 0,
                stream_errors: 0,
            },
            sample_rate,
        ))
    }

    pub(crate) fn start(&mut self, renderer: Renderer) -> Result<()> {
        let Stage::Set(device) = std::mem::replace(&mut self.stage, Stage::Dead) else {
            anyhow::bail!("output backend started twice");
        };
        let buffer_frames = self
            .settings
            .device_buffer_frames
            .clamp(device.buffer_range.0, device.buffer_range.1);
        // The IO callback drains a buffer's worth per wake before the render thread tops up.
        let ahead_frames = ms_to_frames(self.settings.mix_ahead_ms, self.sample_rate);
        let ring_frames = ahead_frames + buffer_frames as usize;
        let (producer, consumer) = RingBuffer::<f32>::new(ring_frames * 2);
        let group = Workgroup::of_device(&device);
        let shared = Arc::clone(&self.shared);
        let period_ns = frames_to_ns(RENDER_CHUNK_FRAMES, self.sample_rate);
        let handle = std::thread::Builder::new()
            .name("audio-render".into())
            .spawn(move || render_loop(renderer, producer, &shared, group, period_ns))
            .context("spawning the render thread")?;
        let _ = self
            .shared
            .control
            .render_thread
            .set(handle.thread().clone());
        self.render = Some(handle);
        self.stage = Stage::Idle {
            consumer,
            since: Instant::now()
                .checked_sub(REOPEN_EVERY)
                .unwrap_or_else(Instant::now),
        };
        let event = self.open(device);
        self.pending.push(event);
        Ok(())
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn stream_errors(&self) -> u64 {
        self.stream_errors
    }

    pub(crate) fn service(&mut self) -> (Window, Vec<Event>) {
        let mut events = std::mem::take(&mut self.pending);
        let notices = &self.shared.notices;
        let lost = if notices.device_died.swap(false, Ordering::AcqRel) {
            Some("the output device went away")
        } else if notices.default_changed.swap(false, Ordering::AcqRel) {
            Some("the system default output changed")
        } else if notices.rate_changed.swap(false, Ordering::AcqRel) {
            Some("the device's sample rate changed")
        } else {
            None
        };
        if let Some(why) = lost
            && matches!(self.stage, Stage::Running { .. })
        {
            self.stream_errors += 1;
            events.push(Event::Lost(why));
            if let Err(e) = self.close() {
                self.stage = Stage::Dead;
                events.push(Event::Dead(format!("{e:#}")));
            }
        }
        if let Stage::Idle { since, .. } = &self.stage
            && since.elapsed() >= REOPEN_EVERY
        {
            match platform::default_output() {
                Ok(device) => {
                    let event = self.open(device);
                    events.push(event);
                }
                Err(e) => {
                    if let Stage::Idle { since, .. } = &mut self.stage {
                        *since = Instant::now();
                    }
                    self.stream_errors += 1;
                    events.push(Event::OpenFailed(format!("{e:#}")));
                }
            }
        }
        (self.take_window(), events)
    }

    /// Opens a stream on `device` around the ring consumer `Idle` holds, and says how it went;
    /// stays `Idle` on failure.
    fn open(&mut self, device: Device) -> Event {
        let Stage::Idle { consumer, .. } = std::mem::replace(&mut self.stage, Stage::Dead) else {
            return Event::Dead("open called without the ring consumer".into());
        };
        if device.sample_rate != self.sample_rate {
            self.sample_rate = device.sample_rate;
            self.shared
                .control
                .pending_rate
                .store(device.sample_rate, Ordering::Release);
            if let Some(thread) = self.shared.control.render_thread.get() {
                thread.unpark();
            }
        }
        let (mut returning, handback) = Returning::new(consumer);
        let shared = Arc::clone(&self.shared);
        let mut last_output_ns = 0u64;
        let on_cycle = move |cycle: Cycle<'_>| {
            if let Some(consumer) = returning.get_mut() {
                io_cycle(cycle, consumer, &shared, &mut last_output_ns);
            }
        };
        match Stream::open(
            &device,
            self.settings.device_buffer_frames,
            &self.shared.notices,
            on_cycle,
        ) {
            Ok(stream) => {
                self.buffer_frames = stream.buffer_frames();
                if let Some(group) = Workgroup::of_device(&device)
                    && let Ok(mut slot) = self.shared.control.new_group.lock()
                {
                    *slot = Some(group);
                    self.shared.control.regroup.store(true, Ordering::Release);
                }
                let listeners = Listeners::arm(&device, Arc::clone(&self.shared.notices));
                self.shared.meters.reset_extrema();
                let event = Event::Opened {
                    device: device.name.clone(),
                    sample_rate: device.sample_rate,
                    buffer_frames: stream.buffer_frames(),
                    mix_ahead_ms: self.settings.mix_ahead_ms,
                    realtime_latency_frames: device.latency_frames + device.safety_frames,
                };
                self.stage = Stage::Running {
                    stream,
                    _listeners: listeners,
                    handback,
                };
                event
            }
            Err(e) => {
                self.stream_errors += 1;
                if let Some(consumer) = take_back(handback) {
                    self.stage = Stage::Idle {
                        consumer,
                        since: Instant::now(),
                    };
                    Event::OpenFailed(format!("{} — {e:#}", device.name))
                } else {
                    Event::Dead(format!(
                        "ring consumer lost while opening {} — {e:#}",
                        device.name
                    ))
                }
            }
        }
    }

    fn close(&mut self) -> Result<()> {
        let Stage::Running {
            stream,
            _listeners: listeners,
            handback,
        } = std::mem::replace(&mut self.stage, Stage::Dead)
        else {
            anyhow::bail!("close without a running stream");
        };
        drop(listeners);
        drop(stream);
        let consumer = take_back(handback).context("the IO closure never returned the ring")?;
        self.stage = Stage::Idle {
            consumer,
            since: Instant::now()
                .checked_sub(REOPEN_EVERY)
                .unwrap_or_else(Instant::now),
        };
        Ok(())
    }

    fn take_window(&mut self) -> Window {
        // The running cycle size, which a host may grant differently from what was asked.
        if let Stage::Running { stream, .. } = &self.stage {
            self.buffer_frames = stream.buffer_frames();
        }
        let m = &self.shared.meters;
        let rate = f64::from(self.sample_rate.max(1));
        let frames_ms = |frames: f64| frames / rate * 1000.0;
        let ns_ms = |ns: u64| ns as f64 / 1e6;
        let lead_min = m.lead_min_ns.swap(i64::MAX, Ordering::Relaxed);
        let ring_min = m.ring_min_frames.swap(usize::MAX, Ordering::Relaxed);
        let overloads_total = self.shared.notices.overloads.load(Ordering::Relaxed);
        let overloads = overloads_total - self.overloads_seen;
        self.overloads_seen = overloads_total;
        let last_overload_ago_ms = (overloads > 0).then(|| {
            let at = self.shared.notices.last_overload_ns.load(Ordering::Relaxed);
            ns_ms(platform::now_ns().saturating_sub(at))
        });
        Window {
            cycles: m.cycles.swap(0, Ordering::Relaxed),
            lead_min_ms: (lead_min != i64::MAX).then(|| lead_min as f64 / 1e6),
            io_wall_max_ms: ns_ms(m.io_wall_max_ns.swap(0, Ordering::Relaxed)),
            gap_max_ms: ns_ms(m.gap_max_ns.swap(0, Ordering::Relaxed)),
            underruns: m.underruns.swap(0, Ordering::Relaxed),
            underrun_ms: frames_ms(m.underrun_frames.swap(0, Ordering::Relaxed) as f64),
            ring_min_ms: (ring_min != usize::MAX).then(|| frames_ms(ring_min as f64)),
            render_wall_max_ms: ns_ms(m.render_wall_max_ns.swap(0, Ordering::Relaxed)),
            render_chunks: m.render_chunks.swap(0, Ordering::Relaxed),
            overloads,
            last_overload_ago_ms,
            cycle_ms: frames_ms(f64::from(self.buffer_frames)),
            chunk_ms: frames_ms(RENDER_CHUNK_FRAMES as f64),
        }
    }
}

impl Drop for DeviceOutput {
    fn drop(&mut self) {
        self.stage = Stage::Dead;
        self.shared.control.stop.store(true, Ordering::Release);
        if let Some(thread) = self.shared.control.render_thread.get() {
            thread.unpark();
        }
        if let Some(handle) = self.render.take() {
            let _ = handle.join();
        }
    }
}

fn take_back(mut handback: Consumer<Consumer<f32>>) -> Option<Consumer<f32>> {
    let deadline = Instant::now() + HANDBACK_WAIT;
    loop {
        if let Ok(consumer) = handback.pop() {
            return Some(consumer);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn ms_to_frames(ms: u32, sample_rate: u32) -> usize {
    (u64::from(ms) * u64::from(sample_rate) / 1000) as usize
}

fn frames_to_ns(frames: usize, sample_rate: u32) -> u64 {
    (frames as u64 * 1_000_000_000) / u64::from(sample_rate.max(1))
}

/// Allocation aborts a debug build inside `f`.
fn no_alloc<R>(f: impl FnOnce() -> R) -> R {
    #[cfg(debug_assertions)]
    {
        assert_no_alloc::assert_no_alloc(f)
    }
    #[cfg(not(debug_assertions))]
    {
        f()
    }
}

/// Copies one device buffer out of the ring. Realtime: no allocation, no lock, no log.
fn io_cycle(cycle: Cycle<'_>, consumer: &mut Consumer<f32>, shared: &Shared, last_ns: &mut u64) {
    no_alloc(|| {
        let entry = platform::now_ns();
        let meters = &shared.meters;
        if cycle.output_time_ns != 0 {
            let lead = cycle.output_time_ns as i64 - entry as i64;
            meters.lead_min_ns.fetch_min(lead, Ordering::Relaxed);
            if *last_ns != 0 {
                let gap = cycle.output_time_ns.saturating_sub(*last_ns);
                meters.gap_max_ns.fetch_max(gap, Ordering::Relaxed);
            }
            *last_ns = cycle.output_time_ns;
        }
        let out = cycle.buffer;
        let need = out.len();
        let banked = consumer.slots() / 2 * 2;
        meters
            .ring_min_frames
            .fetch_min(banked / 2, Ordering::Relaxed);
        let take = banked.min(need);
        if take > 0
            && let Ok(chunk) = consumer.read_chunk(take)
        {
            let (a, b) = chunk.as_slices();
            out[..a.len()].copy_from_slice(a);
            out[a.len()..a.len() + b.len()].copy_from_slice(b);
            chunk.commit_all();
        }
        if take < need {
            out[take..].fill(0.0);
            meters.underruns.fetch_add(1, Ordering::Relaxed);
            meters
                .underrun_frames
                .fetch_add(((need - take) / 2) as u64, Ordering::Relaxed);
        }
        if let Some(thread) = shared.control.render_thread.get() {
            thread.unpark();
        }
        meters.cycles.fetch_add(1, Ordering::Relaxed);
        meters
            .io_wall_max_ns
            .fetch_max(platform::now_ns().saturating_sub(entry), Ordering::Relaxed);
    });
}

/// Keeps the ring full, woken by the IO callback after every cycle and by a timeout.
#[allow(
    clippy::drop_non_drop,
    reason = "on macOS leaving the old workgroup before joining the new one is the point"
)]
fn render_loop(
    mut renderer: Renderer,
    mut producer: Producer<f32>,
    shared: &Shared,
    group: Option<Workgroup>,
    period_ns: u64,
) {
    let _realtime = match platform::set_realtime(period_ns) {
        Ok(realtime) => Some(realtime),
        Err(e) => {
            warn!("audio: the render thread could not go realtime ({e}); raising its QoS instead");
            platform::promote_current_thread();
            None
        }
    };
    let mut joined = group.and_then(|g| match Joined::join(g) {
        Ok(j) => Some(j),
        Err(e) => {
            debug!("audio: the render thread could not join the device's workgroup ({e})");
            None
        }
    });
    let mut chunk = vec![0f32; RENDER_CHUNK_FRAMES * platform::CHANNELS as usize];
    let period = Duration::from_nanos(period_ns);
    let control = &shared.control;
    let meters = &shared.meters;
    loop {
        if control.stop.load(Ordering::Acquire) {
            break;
        }
        let rate = control.pending_rate.swap(0, Ordering::AcqRel);
        if rate != 0 {
            renderer.on_change_sample_rate(rate);
        }
        if control.regroup.swap(false, Ordering::AcqRel) {
            if let Ok(mut slot) = control.new_group.try_lock() {
                if let Some(group) = slot.take() {
                    drop(joined.take());
                    joined = Joined::join(group).ok();
                }
            } else {
                control.regroup.store(true, Ordering::Release);
            }
        }
        while producer.slots() >= chunk.len() {
            let start = platform::now_ns();
            no_alloc(|| {
                renderer.on_start_processing();
                renderer.process(&mut chunk, platform::CHANNELS as u16);
                if let Ok(mut slot) = producer.write_chunk(chunk.len()) {
                    let (a, b) = slot.as_mut_slices();
                    a.copy_from_slice(&chunk[..a.len()]);
                    b.copy_from_slice(&chunk[a.len()..a.len() + b.len()]);
                    slot.commit_all();
                }
            });
            meters.render_chunks.fetch_add(1, Ordering::Relaxed);
            meters
                .render_wall_max_ns
                .fetch_max(platform::now_ns().saturating_sub(start), Ordering::Relaxed);
        }
        std::thread::park_timeout(period);
    }
    drop(joined);
}
