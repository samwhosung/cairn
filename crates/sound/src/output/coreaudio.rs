#![allow(
    unsafe_code,
    reason = "Core Audio, mach and os_workgroup are C APIs with no safe binding for these calls"
)]

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::render_callback::{self, data};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};
use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
    AudioObjectSetPropertyData, kAudioDeviceProcessorOverload, kAudioDevicePropertyBufferFrameSize,
    kAudioDevicePropertyBufferFrameSizeRange, kAudioDevicePropertyDeviceIsAlive,
    kAudioDevicePropertyIOThreadOSWorkgroup, kAudioDevicePropertyLatency,
    kAudioDevicePropertyNominalSampleRate, kAudioDevicePropertySafetyOffset,
    kAudioDevicePropertyScopeOutput, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject,
};
use objc2_core_audio_types::AudioValueRange;

type RenderArgs = render_callback::Args<data::Interleaved<f32>>;

pub(super) const CHANNELS: u32 = 2;

/// `kAudioOutputUnitProperty_CurrentDevice`.
const OUTPUT_UNIT_CURRENT_DEVICE: u32 = 2000;
/// `kAudioUnitProperty_MaximumFramesPerSlice`: a bigger IO buffer than it is refused.
const UNIT_MAXIMUM_FRAMES_PER_SLICE: u32 = 14;
const DEFAULT_MAX_FRAMES_PER_SLICE: u32 = 1156;

#[repr(C)]
struct MachTimebaseInfo {
    numerator: u32,
    denominator: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

/// Nanoseconds on the host clock Core Audio stamps its cycles with.
pub(super) fn now_ns() -> u64 {
    // SAFETY: no arguments, no side effects.
    host_ticks_to_ns(unsafe { mach_absolute_time() })
}

fn host_ticks_to_ns(ticks: u64) -> u64 {
    let (numerator, denominator) = timebase();
    (u128::from(ticks) * u128::from(numerator) / u128::from(denominator)) as u64
}

fn ns_to_host_ticks(ns: u64) -> u64 {
    let (numerator, denominator) = timebase();
    (u128::from(ns) * u128::from(denominator) / u128::from(numerator)) as u64
}

fn timebase() -> (u32, u32) {
    static TIMEBASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo {
            numerator: 0,
            denominator: 0,
        };
        // SAFETY: a plain out-pointer; a failure leaves zeros, guarded below.
        let rc = unsafe { mach_timebase_info(&raw mut info) };
        if rc != 0 || info.denominator == 0 {
            (1, 1)
        } else {
            (info.numerator, info.denominator)
        }
    })
}

#[derive(Clone, Debug)]
pub(super) struct Device {
    pub id: AudioObjectID,
    pub name: String,
    pub sample_rate: u32,
    pub buffer_range: (u32, u32),
    pub latency_frames: u32,
    pub safety_frames: u32,
}

fn addr(selector: AudioObjectPropertySelector, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// # Safety
///
/// `T` must be the C type `selector` carries.
unsafe fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: u32,
) -> Result<T> {
    let address = addr(selector, scope);
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `size` bounds the write into `value`, whose type the caller matches to `selector`.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(value.as_mut_ptr().cast::<c_void>()),
        )
    };
    if status != 0 {
        bail!(
            "property {} read failed (OSStatus {status})",
            fourcc(selector)
        );
    }
    if size as usize != size_of::<T>() {
        bail!(
            "property {} read {size} bytes, not {}",
            fourcc(selector),
            size_of::<T>()
        );
    }
    // SAFETY: the HAL reported writing all of `value`.
    Ok(unsafe { value.assume_init() })
}

fn set<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: u32,
    value: &T,
) -> Result<()> {
    let address = addr(selector, scope);
    // SAFETY: `value` outlives the call and its size is passed with it.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&address),
            0,
            std::ptr::null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast::<c_void>(),
        )
    };
    if status != 0 {
        bail!(
            "property {} write failed (OSStatus {status})",
            fourcc(selector)
        );
    }
    Ok(())
}

fn fourcc(selector: u32) -> String {
    selector
        .to_be_bytes()
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '?' })
        .collect()
}

pub(super) fn default_output() -> Result<Device> {
    // SAFETY: the default output device is an `AudioObjectID`.
    let id: AudioObjectID = unsafe {
        get(
            kAudioObjectSystemObject as AudioObjectID,
            kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal,
        )
    }
    .context("no default output device")?;
    if id == 0 {
        bail!("no default output device");
    }
    describe(id)
}

fn describe(id: AudioObjectID) -> Result<Device> {
    let name = coreaudio::audio_unit::macos_helpers::get_device_name(id)
        .unwrap_or_else(|_| format!("device {id}"));
    // SAFETY: the nominal sample rate is a `Float64`.
    let rate: f64 = unsafe {
        get(
            id,
            kAudioDevicePropertyNominalSampleRate,
            kAudioObjectPropertyScopeGlobal,
        )
    }
    .context("device sample rate")?;
    if !(8000.0..=384_000.0).contains(&rate) {
        bail!("device {name} reports an absurd sample rate {rate}");
    }
    // SAFETY: the buffer frame size range is an `AudioValueRange`.
    let range: AudioValueRange = unsafe {
        get(
            id,
            kAudioDevicePropertyBufferFrameSizeRange,
            kAudioObjectPropertyScopeGlobal,
        )
    }
    .context("device buffer range")?;
    // SAFETY: the latency and the safety offset are `UInt32` frame counts.
    let frames = |selector| unsafe {
        get::<u32>(id, selector, kAudioDevicePropertyScopeOutput).unwrap_or(0)
    };
    Ok(Device {
        id,
        name,
        sample_rate: rate.round() as u32,
        buffer_range: (
            range.mMinimum.max(1.0) as u32,
            range.mMaximum.max(1.0) as u32,
        ),
        latency_frames: frames(kAudioDevicePropertyLatency),
        safety_frames: frames(kAudioDevicePropertySafetyOffset),
    })
}

/// One IO cycle: the interleaved stereo buffer to fill, and when its first frame is handed to the
/// hardware on the [`now_ns`] clock.
pub(super) struct Cycle<'a> {
    pub buffer: &'a mut [f32],
    pub output_time_ns: Option<u64>,
}

/// A running stream; dropping it stops the unit and frees the callback before it returns.
pub(super) struct Stream {
    unit: AudioUnit,
    buffer_frames: u32,
}

impl Stream {
    /// The IO buffer the device actually runs, read back after the open.
    pub(super) fn buffer_frames(&self) -> u32 {
        self.buffer_frames
    }

    /// `on_cycle` runs on the device's realtime thread: it must never block, allocate or log.
    /// Every notice here comes from [`Listeners`].
    pub(super) fn open<F>(
        device: &Device,
        buffer_frames: u32,
        _notices: &Arc<Notices>,
        mut on_cycle: F,
    ) -> Result<Self>
    where
        F: FnMut(Cycle<'_>) + Send + 'static,
    {
        let buffer_frames = buffer_frames.clamp(device.buffer_range.0, device.buffer_range.1);
        // A per-client property: this sizes our IO cycle and no other process's.
        set(
            device.id,
            kAudioDevicePropertyBufferFrameSize,
            kAudioObjectPropertyScopeGlobal,
            &buffer_frames,
        )
        .context("device buffer size")?;
        let mut unit = AudioUnit::new(IOType::HalOutput).context("HAL output unit")?;
        unit.set_property(
            OUTPUT_UNIT_CURRENT_DEVICE,
            Scope::Global,
            Element::Output,
            Some(&device.id),
        )
        .context("pinning the unit to the device")?;
        let format = StreamFormat {
            sample_rate: f64::from(device.sample_rate),
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT | LinearPcmFlags::IS_PACKED,
            channels: CHANNELS,
        };
        unit.set_stream_format(format, Scope::Input, Element::Output)
            .context("stream format")?;
        unit.set_property(
            UNIT_MAXIMUM_FRAMES_PER_SLICE,
            Scope::Global,
            Element::Output,
            Some(&buffer_frames.max(DEFAULT_MAX_FRAMES_PER_SLICE)),
        )
        .context("maximum frames per slice")?;
        unit.set_render_callback(move |args: RenderArgs| {
            let RenderArgs {
                data, time_stamp, ..
            } = args;
            let output_time_ns =
                (time_stamp.mHostTime != 0).then(|| host_ticks_to_ns(time_stamp.mHostTime));
            on_cycle(Cycle {
                buffer: data.buffer,
                output_time_ns,
            });
            Ok(())
        })
        .context("render callback")?;
        unit.start().context("starting the unit")?;
        // SAFETY: the buffer frame size is a `UInt32`.
        let buffer_frames = unsafe {
            get::<u32>(
                device.id,
                kAudioDevicePropertyBufferFrameSize,
                kAudioObjectPropertyScopeGlobal,
            )
        }
        .unwrap_or(buffer_frames);
        Ok(Self {
            unit,
            buffer_frames,
        })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.unit.stop();
    }
}

#[derive(Default)]
pub(super) struct Notices {
    pub default_changed: AtomicBool,
    pub device_died: AtomicBool,
    pub rate_changed: AtomicBool,
    pub overloads: AtomicU64,
    pub last_overload_ns: AtomicU64,
}

unsafe extern "C-unwind" fn on_notice(
    _object: AudioObjectID,
    count: u32,
    addresses: NonNull<AudioObjectPropertyAddress>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the `Notices` registered with the listener, which outlives it.
    let notices = unsafe { &*(client as *const Notices) };
    for i in 0..count as usize {
        // SAFETY: Core Audio hands `count` valid addresses.
        let address = unsafe { *addresses.as_ptr().add(i) };
        match address.mSelector {
            s if s == kAudioHardwarePropertyDefaultOutputDevice => {
                notices.default_changed.store(true, Ordering::Release);
            }
            s if s == kAudioDevicePropertyDeviceIsAlive => {
                notices.device_died.store(true, Ordering::Release);
            }
            s if s == kAudioDevicePropertyNominalSampleRate => {
                notices.rate_changed.store(true, Ordering::Release);
            }
            s if s == kAudioDeviceProcessorOverload => {
                notices.last_overload_ns.store(now_ns(), Ordering::Relaxed);
                notices.overloads.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
    0
}

pub(super) struct Listeners {
    notices: Arc<Notices>,
    armed: Vec<(AudioObjectID, AudioObjectPropertyAddress)>,
}

impl Listeners {
    pub(super) fn arm(device: &Device, notices: Arc<Notices>) -> Self {
        let client = Arc::as_ptr(&notices) as *mut c_void;
        let global = kAudioObjectPropertyScopeGlobal;
        let wanted = [
            (
                kAudioObjectSystemObject as AudioObjectID,
                addr(kAudioHardwarePropertyDefaultOutputDevice, global),
            ),
            (device.id, addr(kAudioDevicePropertyDeviceIsAlive, global)),
            (
                device.id,
                addr(kAudioDevicePropertyNominalSampleRate, global),
            ),
            (device.id, addr(kAudioDeviceProcessorOverload, global)),
        ];
        let mut armed = Vec::with_capacity(wanted.len());
        for (object, address) in wanted {
            // SAFETY: `client` stays valid while `self` holds the Arc; removed in `Drop`.
            let status = unsafe {
                AudioObjectAddPropertyListener(
                    object,
                    NonNull::from(&address),
                    Some(on_notice),
                    client,
                )
            };
            if status == 0 {
                armed.push((object, address));
            } else {
                bevy::log::warn!(
                    "audio: listener {} on object {object} refused (OSStatus {status})",
                    fourcc(address.mSelector)
                );
            }
        }
        Self { notices, armed }
    }
}

impl Drop for Listeners {
    fn drop(&mut self) {
        let client = Arc::as_ptr(&self.notices) as *mut c_void;
        for (object, address) in self.armed.drain(..) {
            // SAFETY: mirrors the registration; a dead device may refuse, and its listeners
            // die with it.
            let _ = unsafe {
                AudioObjectRemovePropertyListener(
                    object,
                    NonNull::from(&address),
                    Some(on_notice),
                    client,
                )
            };
        }
    }
}

type OsWorkgroup = *mut c_void;

/// `os_workgroup_join_token_s`, sized generously: the OS writes into it and we only carry it back.
#[repr(C)]
struct JoinToken {
    sig: u32,
    opaque: [u8; 60],
}

unsafe extern "C" {
    fn os_workgroup_join(wg: OsWorkgroup, token_out: *mut JoinToken) -> i32;
    fn os_workgroup_leave(wg: OsWorkgroup, token: *mut JoinToken);
    fn os_release(object: *mut c_void);
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// The device's IO-thread workgroup: the threads the scheduler serves toward its deadline.
pub(super) struct Workgroup(OsWorkgroup);

// SAFETY: an os_workgroup is a thread-safe OS object, only joined, left and released.
unsafe impl Send for Workgroup {}

impl Workgroup {
    pub(super) fn of_device(device: &Device) -> Option<Self> {
        // SAFETY: the IO thread's workgroup is an `os_workgroup_t`.
        let wg: OsWorkgroup = unsafe {
            get(
                device.id,
                kAudioDevicePropertyIOThreadOSWorkgroup,
                kAudioObjectPropertyScopeGlobal,
            )
        }
        .ok()?;
        (!wg.is_null()).then_some(Self(wg))
    }
}

impl Drop for Workgroup {
    fn drop(&mut self) {
        // SAFETY: the property hands over a retained object we own.
        unsafe { os_release(self.0) };
    }
}

/// The render thread's membership, left on drop on the same thread.
pub(super) struct Joined {
    group: Workgroup,
    token: Box<JoinToken>,
    stays_on_its_thread: std::marker::PhantomData<*const ()>,
}

impl Joined {
    /// From the thread that renders, after [`set_realtime`].
    pub(super) fn join(group: Workgroup) -> Result<Self> {
        let mut token = Box::new(JoinToken {
            sig: 0,
            opaque: [0; 60],
        });
        // SAFETY: a live retained group, and a token that is ours to hold.
        let rc = unsafe { os_workgroup_join(group.0, &raw mut *token) };
        if rc != 0 {
            bail!("os_workgroup_join failed ({rc})");
        }
        Ok(Self {
            group,
            token,
            stays_on_its_thread: std::marker::PhantomData,
        })
    }
}

impl Drop for Joined {
    fn drop(&mut self) {
        // SAFETY: the token `join` filled, on the thread that joined.
        unsafe { os_workgroup_leave(self.group.0, &raw mut *self.token) };
    }
}

pub(super) struct Realtime;

pub(super) fn set_realtime(period_ns: u64) -> Result<Realtime> {
    let period = ns_to_host_ticks(period_ns) as u32;
    let policy = libc::thread_time_constraint_policy {
        period,
        computation: (period / 10).max(1),
        constraint: (period / 2).max(2),
        preemptible: 1,
    };
    // SAFETY: the documented flavor's layout, with its documented count.
    let rc = unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            std::ptr::from_ref(&policy) as libc::thread_policy_t,
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        )
    };
    if rc != 0 {
        return Err(anyhow!("thread_policy_set(TIME_CONSTRAINT) failed ({rc})"));
    }
    Ok(Realtime)
}

/// Raises the calling thread to the user-interactive quality of service: a spawned thread does
/// not inherit its spawner's class.
pub(crate) fn promote_current_thread() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    // SAFETY: a plain call on the calling thread.
    let rc = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
    if rc != 0 {
        bevy::log::warn_once!("thread QoS promotion failed (rc={rc})");
    }
}
