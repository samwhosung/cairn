//! A look-ahead brickwall limiter on the mix, stereo-linked, so kira's hard clamp never shapes
//! the waveform. Every 1.12 effect is mastered to full scale and the mix is a plain sum, so two
//! close sounds already ask for more than full scale; the client itself clipped.
//!
//! Per frame the required gain is `CEILING / peak`; a sliding minimum of it over the look-ahead,
//! averaged over the same span, is at most the required gain of the frame leaving the delay line,
//! so the output never overshoots. The gain then returns to unity over [`RELEASE_MS`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

use crate::meter::MixLevel;

/// A hair under full scale, so the renderer's clamp never fires.
const CEILING: f32 = 0.99;
/// Both the anticipation window and the latency added; longer than a percussive attack.
const LOOKAHEAD_MS: f32 = 2.0;
/// Long enough not to modulate at audio rate, short enough to leave no quiet hole behind a burst.
const RELEASE_MS: f32 = 120.0;

#[cfg(test)]
pub(crate) fn ceiling() -> f32 {
    CEILING
}

/// `enabled` is read once a block; the delay line runs either way, so a toggle fades.
pub(crate) fn install(
    builder: &mut kira::track::MainTrackBuilder,
    level: &Arc<MixLevel>,
    enabled: &Arc<AtomicBool>,
) {
    builder.add_effect(LimiterBuilder {
        level: Arc::clone(level),
        enabled: Arc::clone(enabled),
    });
}

struct LimiterBuilder {
    level: Arc<MixLevel>,
    enabled: Arc<AtomicBool>,
}

impl EffectBuilder for LimiterBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (
            Box::new(Limiter {
                level: self.level,
                enabled: self.enabled,
                core: LimiterCore::new(),
            }),
            (),
        )
    }
}

struct Limiter {
    level: Arc<MixLevel>,
    enabled: Arc<AtomicBool>,
    core: LimiterCore,
}

impl Effect for Limiter {
    fn init(&mut self, sample_rate: u32, _internal_buffer_size: usize) {
        self.core.resize(sample_rate);
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.core.resize(sample_rate);
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        let bypass = !self.enabled.load(Ordering::Relaxed);
        let mut deepest = 1.0f32;
        for f in input.iter_mut() {
            let (out, gain) = self.core.step(*f, bypass);
            *f = out;
            deepest = deepest.min(gain);
        }
        self.level.gain(deepest);
    }
}

pub(crate) struct LimiterCore {
    delay: Vec<Frame>,
    delay_w: usize,
    /// `(sample index, required gain)`, increasing: the sliding minimum.
    win: VecDeque<(u64, f32)>,
    n: u64,
    hist: Vec<f32>,
    hist_w: usize,
    hist_sum: f64,
    gain: f32,
    release: f32,
    lookahead: usize,
}

impl LimiterCore {
    pub(crate) fn new() -> Self {
        Self {
            delay: Vec::new(),
            delay_w: 0,
            win: VecDeque::new(),
            n: 0,
            hist: Vec::new(),
            hist_w: 0,
            hist_sum: 0.0,
            gain: 1.0,
            release: 0.0,
            lookahead: 0,
        }
    }

    /// The one allocating call; kira runs it off the render path.
    pub(crate) fn resize(&mut self, sample_rate: u32) {
        let rate = sample_rate.max(1) as f32;
        self.lookahead = ((LOOKAHEAD_MS / 1000.0 * rate).round() as usize).max(1);
        self.delay.clear();
        self.delay.resize(self.lookahead, Frame::ZERO);
        self.delay_w = 0;
        self.win.clear();
        self.win.reserve(self.lookahead + 2);
        self.n = 0;
        self.hist.clear();
        self.hist.resize(self.lookahead, 1.0);
        self.hist_w = 0;
        self.hist_sum = self.lookahead as f64;
        self.gain = 1.0;
        self.release = (-1000.0 / (RELEASE_MS * rate)).exp();
    }

    /// One frame in, one frame out with the gain applied to it. Bypass steers the target to unity
    /// rather than skipping the delay.
    pub(crate) fn step(&mut self, input: Frame, bypass: bool) -> (Frame, f32) {
        let peak = input.left.abs().max(input.right.abs());
        let required = if peak > CEILING { CEILING / peak } else { 1.0 };
        while self.win.back().is_some_and(|&(_, g)| g >= required) {
            self.win.pop_back();
        }
        self.win.push_back((self.n, required));
        let oldest = self.n.saturating_sub(self.lookahead as u64);
        while self.win.front().is_some_and(|&(i, _)| i < oldest) {
            self.win.pop_front();
        }
        let window_min = self.win.front().map_or(1.0, |&(_, g)| g);
        self.hist_sum += f64::from(window_min) - f64::from(self.hist[self.hist_w]);
        self.hist[self.hist_w] = window_min;
        self.hist_w = (self.hist_w + 1) % self.lookahead;
        let smoothed = if bypass {
            1.0
        } else {
            (self.hist_sum / self.lookahead as f64) as f32
        };
        let recovered = 1.0 - (1.0 - self.gain) * self.release;
        self.gain = smoothed.min(recovered);
        let out = self.delay[self.delay_w] * self.gain;
        self.delay[self.delay_w] = input;
        self.delay_w = (self.delay_w + 1) % self.lookahead;
        self.n += 1;
        (out, self.gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn core() -> LimiterCore {
        let mut c = LimiterCore::new();
        c.resize(RATE);
        c
    }

    fn peak(frames: &[Frame]) -> f32 {
        frames
            .iter()
            .map(|f| f.left.abs().max(f.right.abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn nothing_passes_the_ceiling_at_any_overload() {
        for n in [2.0f32, 5.0, 12.0, 40.0] {
            let mut c = core();
            let out: Vec<Frame> = (0..RATE / 5)
                .map(|i| {
                    let s = n * (i as f32 * 440.0 * std::f32::consts::TAU / RATE as f32).sin();
                    c.step(Frame::from_mono(s), false).0
                })
                .collect();
            assert!(peak(&out) <= CEILING + 1e-5, "{n}x leaked {}", peak(&out));
        }
        let mut c = core();
        let mut frames = vec![Frame::ZERO; 500];
        frames[300] = Frame::from_mono(5.0);
        let out: Vec<Frame> = frames.iter().map(|f| c.step(*f, false).0).collect();
        assert!(peak(&out) <= CEILING + 1e-5);
    }

    #[test]
    fn quiet_material_is_a_pure_delay() {
        let mut c = core();
        let frames: Vec<Frame> = (0..2000)
            .map(|i| {
                Frame::from_mono(
                    0.5 * (i as f32 * 220.0 * std::f32::consts::TAU / RATE as f32).sin(),
                )
            })
            .collect();
        let out: Vec<Frame> = frames.iter().map(|f| c.step(*f, false).0).collect();
        for (i, f) in out.iter().enumerate().skip(c.lookahead) {
            assert!((f.left - frames[i - c.lookahead].left).abs() < 1e-6);
        }
    }

    #[test]
    fn the_gain_recovers_and_bypass_fades() {
        let mut c = core();
        for _ in 0..RATE / 20 {
            c.step(Frame::from_mono(5.0), false);
        }
        assert!(c.gain < 0.3);
        let mut last = c.gain;
        for _ in 0..RATE / 2 {
            let (_, g) = c.step(Frame::from_mono(5.0), true);
            assert!(g - last < 0.01, "stepped from {last} to {g}");
            last = g;
        }
        assert!(last > 0.98);
    }
}
