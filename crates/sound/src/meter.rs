//! The mix's level on the audio thread, read and reset from the main thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;

/// Peaks are kept as the bits of non-negative floats, which order as the floats do, so atomic
/// max and min on the bits are max and min on the values.
#[derive(Debug)]
pub struct MixLevel {
    peak_bits: AtomicU32,
    over: AtomicU64,
    reduction_bits: AtomicU32,
    nonfinite: AtomicU64,
}

impl Default for MixLevel {
    fn default() -> Self {
        Self {
            peak_bits: AtomicU32::new(0),
            over: AtomicU64::new(0),
            reduction_bits: AtomicU32::new(1.0f32.to_bits()),
            nonfinite: AtomicU64::new(0),
        }
    }
}

/// One window of [`MixLevel`].
#[derive(Clone, Copy, Debug)]
pub struct LevelReading {
    /// The largest `|sample|` the summed mix asked for; over `1.0` did not fit.
    pub peak: f32,
    /// Samples past full scale.
    pub over: u64,
    /// The limiter's deepest gain, `1.0` when it never engaged.
    pub reduction: f32,
    /// NaN or infinite samples: a defect upstream, never a loud passage.
    pub nonfinite: u64,
}

impl MixLevel {
    fn block(&self, peak: f32, over: u64, nonfinite: u64) {
        self.peak_bits.fetch_max(peak.to_bits(), Ordering::Relaxed);
        if over > 0 {
            self.over.fetch_add(over, Ordering::Relaxed);
        }
        if nonfinite > 0 {
            self.nonfinite.fetch_add(nonfinite, Ordering::Relaxed);
        }
    }

    pub(crate) fn gain(&self, gain: f32) {
        self.reduction_bits
            .fetch_min(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn take(&self) -> LevelReading {
        LevelReading {
            peak: f32::from_bits(self.peak_bits.swap(0, Ordering::Relaxed)),
            over: self.over.swap(0, Ordering::Relaxed),
            reduction: f32::from_bits(
                self.reduction_bits
                    .swap(1.0f32.to_bits(), Ordering::Relaxed),
            ),
            nonfinite: self.nonfinite.swap(0, Ordering::Relaxed),
        }
    }
}

pub(crate) fn install(builder: &mut kira::track::MainTrackBuilder, level: &Arc<MixLevel>) {
    builder.add_effect(MeterBuilder {
        level: Arc::clone(level),
    });
}

struct MeterBuilder {
    level: Arc<MixLevel>,
}

impl EffectBuilder for MeterBuilder {
    type Handle = ();
    fn build(self) -> (Box<dyn Effect>, Self::Handle) {
        (Box::new(Meter { level: self.level }), ())
    }
}

struct Meter {
    level: Arc<MixLevel>,
}

impl Effect for Meter {
    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        let mut peak = 0.0f32;
        let mut over = 0u64;
        let mut nonfinite = 0u64;
        for f in input.iter() {
            for mag in [f.left.abs(), f.right.abs()] {
                // A NaN would vanish into `max` and fail the over-scale compare.
                if mag.is_finite() {
                    peak = peak.max(mag);
                    over += u64::from(mag > 1.0);
                } else {
                    nonfinite += 1;
                }
            }
        }
        self.level.block(peak, over, nonfinite);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_its_own_and_nan_is_counted_not_swallowed() {
        let level = MixLevel::default();
        level.block(0.5, 0, 0);
        level.block(2.0, 2, 0);
        level.gain(0.25);
        let r = level.take();
        assert_eq!((r.peak, r.over, r.reduction), (2.0, 2, 0.25));
        let r = level.take();
        assert_eq!((r.peak, r.over, r.reduction), (0.0, 0, 1.0));

        let mut meter = Meter {
            level: Arc::new(MixLevel::default()),
        };
        let mut block = [
            Frame::from_mono(f32::NAN),
            Frame::from_mono(0.5),
            Frame::from_mono(f32::INFINITY),
        ];
        meter.process(
            &mut block,
            1.0 / 44_100.0,
            &kira::info::MockInfoBuilder::new().build(),
        );
        let r = meter.level.take();
        assert_eq!((r.nonfinite, r.peak, r.over), (4, 0.5, 0));
    }
}
