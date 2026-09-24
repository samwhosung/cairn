//! What the output says about itself, and the offline clock that renders the mix in step with
//! the game.

use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use bevy::prelude::*;

use crate::meter::LevelReading;
use crate::mix_tap::wav_header;
use crate::output::Window;
use crate::plugin::SoundOutput;

/// How much time one report sums up.
const REPORT_EVERY: Duration = Duration::from_secs(5);

/// Services the output every frame, and every [`REPORT_EVERY`] says how it kept up and how loud
/// the mix asked to be.
pub(crate) fn poll_mix_health(
    mut out: NonSendMut<'_, SoundOutput>,
    time: Res<'_, Time>,
    mut exit: MessageReader<'_, '_, AppExit>,
    mut since: Local<'_, Duration>,
    mut last_refused: Local<'_, u64>,
    mut peak_voices: Local<'_, usize>,
) {
    *peak_voices = (*peak_voices).max(out.channel_count());
    let Some(mixer) = out.mixer.as_mut() else {
        return;
    };
    let health = mixer.poll_health();
    *since += time.delta();
    let exiting = exit.read().next().is_some();
    if *since < REPORT_EVERY && !exiting {
        return;
    }
    *since = Duration::ZERO;
    let peak = mixer.take_health_peak();
    let level = mixer.take_level();
    let rate = mixer.sample_rate();
    report_output(mixer.take_output_window(), peak);
    let refused = health.voices_refused - *last_refused;
    *last_refused = health.voices_refused;
    if refused > 0 {
        warn!("audio: {refused} 3-D sound(s) never played: the spatial-voice arena was full");
    }
    report_level(level, std::mem::take(&mut *peak_voices), rate);
}

fn report_output(w: Window, peak_load: f32) {
    if w.cycles == 0 {
        return;
    }
    let lead = w
        .lead_min_ms
        .map_or_else(|| "n/a".to_string(), |v| format!("{v:.1} ms"));
    let ring = w
        .ring_min_ms
        .map_or_else(|| "n/a".to_string(), |v| format!("{v:.0} ms"));
    let shape = format!(
        "{} cycles: lead ≥ {lead}, IO copy ≤ {:.2} ms, cycle gap ≤ {:.1} ms (nominal {:.1}), \
         render ≤ {:.2} ms per {:.1} ms chunk (peak load {:.0}%), ring ≥ {ring}",
        w.cycles,
        w.io_wall_max_ms,
        w.gap_max_ms,
        w.cycle_ms,
        w.render_wall_max_ms,
        w.chunk_ms,
        peak_load * 100.0,
    );
    if w.underruns > 0 {
        warn!(
            "audio: {} underrun(s), ~{:.0} ms of silence reached the device. {shape}",
            w.underruns, w.underrun_ms,
        );
    }
    if w.overloads > 0 {
        warn!(
            "audio: {} device overload(s): an IO cycle ran past its deadline. {shape}",
            w.overloads
        );
    } else if w.lead_min_ms.is_some_and(|l| l < 0.0)
        || w.io_wall_max_ms > w.cycle_ms * 0.5
        || w.gap_max_ms > w.cycle_ms * 1.5
    {
        warn!("audio: IO cycle strain without an overload. {shape}");
    } else {
        debug!("audio: output steady. {shape}");
    }
}

fn report_level(level: LevelReading, voices: usize, rate: Option<u32>) {
    if level.nonfinite > 0 {
        error!(
            "audio: {} non-finite sample(s) reached the mix: a defect upstream, which the \
             limiter cannot catch",
            level.nonfinite,
        );
    }
    if level.over == 0 {
        debug!(
            "audio: mix peak {:.2} of full scale, {voices} voice(s) at most",
            level.peak
        );
        return;
    }
    let over = match rate {
        Some(r) => format!(
            "for ~{:.0} ms",
            level.over as f64 / 2.0 / f64::from(r) * 1000.0
        ),
        None => format!("across {} samples", level.over),
    };
    warn!(
        "audio: the mix asked for {:.2}x full scale ({:+.1} dBFS) {over} of the last {} s with \
         {voices} voice(s) live at most; the limiter pulled up to {:.1} dB",
        level.peak,
        20.0 * level.peak.max(1e-6).log10(),
        REPORT_EVERY.as_secs(),
        20.0 * level.reduction.max(1e-6).log10(),
    );
}

/// The offline mix's position on the game clock, and where its frames go.
pub(crate) struct OfflineClock {
    sample_rate: u32,
    rendered: u64,
    record: Option<(std::fs::File, u64)>,
}

impl OfflineClock {
    pub(crate) fn new(sample_rate: u32, record: Option<&Path>) -> Self {
        let record = record.and_then(|path| match std::fs::File::create(path) {
            Ok(mut file) => {
                let _ = file.write_all(&wav_header(sample_rate, 0));
                Some((file, 0))
            }
            Err(e) => {
                warn!("sound: cannot record to {}: {e}", path.display());
                None
            }
        });
        Self {
            sample_rate,
            rendered: 0,
            record,
        }
    }
}

/// Offline, renders the mix up to the game's elapsed time, after every sound this frame started.
pub(crate) fn render_offline(mut out: NonSendMut<'_, SoundOutput>, time: Res<'_, Time>) {
    let out = &mut *out;
    let (Some(clock), Some(mixer)) = (out.offline.as_mut(), out.mixer.as_mut()) else {
        return;
    };
    let due = (time.elapsed_secs_f64() * f64::from(clock.sample_rate)).round() as u64;
    let frames = due.saturating_sub(clock.rendered);
    if frames == 0 {
        return;
    }
    let samples = mixer.render(frames as usize);
    clock.rendered = due;
    if let Some((file, written)) = clock.record.as_mut() {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let ok = file.write_all(&bytes).is_ok() && {
            *written += samples.len() as u64;
            file.seek(SeekFrom::Start(0)).is_ok()
                && file
                    .write_all(&wav_header(clock.sample_rate, *written))
                    .is_ok()
                && file.seek(SeekFrom::End(0)).is_ok()
        };
        if !ok {
            warn!("sound: the offline recording failed; it stops here");
            clock.record = None;
        }
    }
}
