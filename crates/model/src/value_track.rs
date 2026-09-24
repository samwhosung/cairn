use crate::particles::KeyBudget;
use crate::{le_u16, le_u32};

pub trait TrackValue: Copy {
    /// What an empty track reads.
    const ZERO: Self;
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl TrackValue for f32 {
    const ZERO: Self = 0.0;
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl TrackValue for [f32; 3] {
    const ZERO: Self = [0.0; 3];
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
    }
}

/// A keyed track. A sequence track's keys run from the first sequence's start; a
/// global-sequence track keeps its own clock.
#[derive(Debug, Clone, Default)]
pub struct ValueTrack<V = f32> {
    /// `(ms, value)` in file order.
    pub keys: Vec<(u32, V)>,
    /// The track's interpolation word: `0` steps, anything else is linear.
    pub interp: u16,
}

impl<V: TrackValue> ValueTrack<V> {
    pub(crate) fn constant(v: V) -> Self {
        Self {
            keys: vec![(0, v)],
            interp: 0,
        }
    }

    pub fn first(&self) -> V {
        self.keys.first().map_or(V::ZERO, |&(_, v)| v)
    }

    /// The last key at or before `ms`, the first key's value before it.
    pub fn step_ms(&self, ms: f32) -> V {
        let mut v = self.keys.first().map_or(V::ZERO, |&(_, v)| v);
        for &(t, val) in &self.keys {
            if (t as f32) <= ms {
                v = val;
            } else {
                break;
            }
        }
        v
    }

    /// The client's per-frame sampling: [`Self::step_ms`] when [`Self::interp`] is 0, else linear
    /// between the keys around `ms`, holding the last key past the end and extrapolating the first
    /// segment backward below the first key.
    pub fn sampled_ms(&self, ms: f32) -> V {
        if self.interp == 0 {
            return self.step_ms(ms);
        }
        let n = self.keys.len();
        if n <= 1 {
            return self.first();
        }
        let k = self
            .keys
            .iter()
            .rposition(|&(t, _)| (t as f32) <= ms)
            .unwrap_or(0);
        if k + 1 == n {
            return self.keys[n - 1].1;
        }
        let (t0, v0) = self.keys[k];
        let (t1, v1) = self.keys[k + 1];
        let span = (t1.saturating_sub(t0)).max(1) as f32;
        V::lerp(v0, v1, (ms - t0 as f32) / span)
    }

    /// Linear between neighbouring keys, holding the first and last key outside them.
    pub fn sample_ms(&self, ms: f32) -> V {
        let Some(&(t0, v0)) = self.keys.first() else {
            return V::ZERO;
        };
        if ms <= t0 as f32 {
            return v0;
        }
        for w in self.keys.windows(2) {
            let ((ta, va), (tb, vb)) = (w[0], w[1]);
            if ms < tb as f32 {
                let span = tb.saturating_sub(ta).max(1) as f32;
                return V::lerp(va, vb, (ms - ta as f32) / span);
            }
        }
        self.keys.last().map_or(V::ZERO, |&(_, v)| v)
    }
}

impl ValueTrack<f32> {
    /// The largest key; `f32::MIN` without keys.
    pub fn peak(&self) -> f32 {
        self.keys.iter().fold(f32::MIN, |m, &(_, v)| m.max(v))
    }
}

pub(crate) fn rebase_keys_to_band<V: Copy>(keys: &mut Vec<(u32, V)>, start: u32, end: u32) {
    if keys.is_empty() || (start == 0 && keys.last().is_some_and(|&(t, _)| t <= end)) {
        return;
    }
    let mut out: Vec<(u32, V)> = Vec::with_capacity(keys.len());
    for &(t, v) in keys.iter() {
        if t <= start {
            match out.first_mut() {
                Some(first) if first.0 == 0 => *first = (0, v),
                _ => out.insert(0, (0, v)),
            }
        } else if t < end {
            out.push((t - start, v));
        } else {
            out.push((end.saturating_sub(start), v));
            break;
        }
    }
    *keys = out;
}

pub(crate) fn seq0_band(bytes: &[u8]) -> (u32, u32) {
    let (n_seq, o_seq) = (le_u32(bytes, 0x1c) as usize, le_u32(bytes, 0x20) as usize);
    if n_seq > 0 && o_seq + 0x44 <= bytes.len() {
        (le_u32(bytes, o_seq + 4), le_u32(bytes, o_seq + 8))
    } else {
        (0, u32::MAX)
    }
}

struct TrackArrays {
    gseq: u16,
    count: usize,
    times_at: usize,
    values_at: usize,
}

fn track_arrays(b: &[u8], track: usize) -> Option<TrackArrays> {
    if track + 0x1c > b.len() {
        return None;
    }
    let tn = le_u32(b, track + 0x0c) as usize;
    let vn = le_u32(b, track + 0x14) as usize;
    let arrays = TrackArrays {
        gseq: le_u16(b, track + 0x02),
        count: tn.min(vn),
        times_at: le_u32(b, track + 0x10) as usize,
        values_at: le_u32(b, track + 0x18) as usize,
    };
    (arrays.count > 0 && arrays.times_at + arrays.count * 4 <= b.len()).then_some(arrays)
}

pub(crate) fn track_keys_with<V: TrackValue>(
    b: &[u8],
    track: usize,
    (default, band): (V, (u32, u32)),
    elem: usize,
    read: impl Fn(&[u8], usize) -> V,
    budget: &KeyBudget,
) -> ValueTrack<V> {
    let Some(arrays) = track_arrays(b, track) else {
        return ValueTrack::constant(default);
    };
    let n = arrays.count;
    if arrays.values_at + n * elem > b.len() || !budget.try_spend(n) {
        return ValueTrack::constant(default);
    }
    let mut keys: Vec<(u32, V)> = (0..n)
        .map(|i| {
            let t = le_u32(b, arrays.times_at + i * 4);
            (t, read(b, arrays.values_at + i * elem))
        })
        .collect();
    if arrays.gseq == 0xffff {
        rebase_keys_to_band(&mut keys, band.0, band.1);
    }
    ValueTrack {
        keys,
        interp: le_u16(b, track),
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn linear_sampling_holds_both_ends() {
        let t = ValueTrack {
            keys: vec![(0, 0.0), (33, 0.0), (67, 100.0), (100, 200.0), (133, 0.0)],
            interp: 1,
        };
        assert_eq!(t.sample_ms(-5.0), 0.0);
        assert!((t.sample_ms(50.0) - 50.0).abs() < 1.0);
        assert_eq!(t.sample_ms(100.0), 200.0);
        assert!((t.sample_ms(116.5) - 100.0).abs() < 1.0);
        assert_eq!(t.sample_ms(500.0), 0.0);
        assert_eq!(t.peak(), 200.0);
        assert_eq!(ValueTrack::constant(7.5).sample_ms(9999.0), 7.5);
        assert_eq!(ValueTrack::<f32>::default().sample_ms(10.0), 0.0);
    }

    #[test]
    fn a_step_track_is_silent_until_its_key() {
        let t = ValueTrack {
            keys: vec![(0, 0.0), (67, 30.0)],
            interp: 0,
        };
        assert_eq!(t.step_ms(66.0), 0.0);
        assert_eq!(t.step_ms(67.0), 30.0);
        assert_eq!(t.sampled_ms(66.0), 0.0);
        assert_eq!(t.step_ms(1500.0), 30.0);
    }

    #[test]
    fn the_clients_sampling_extrapolates_below_the_first_key() {
        let ramp = ValueTrack {
            keys: vec![(100, 10.0), (200, 110.0), (300, 0.0)],
            interp: 1,
        };
        assert!((ramp.sampled_ms(150.0) - 60.0).abs() < 1e-4);
        assert_eq!(ramp.sampled_ms(999.0), 0.0);
        assert!((ramp.sampled_ms(0.0) + 90.0).abs() < 1e-4);
    }

    #[test]
    fn a_colour_track_lerps_each_channel() {
        let t = ValueTrack {
            keys: vec![(0, [1.0, 0.0, 0.0]), (100, [0.0, 1.0, 0.5])],
            interp: 1,
        };
        let mid = t.sample_ms(50.0);
        assert!((mid[0] - 0.5).abs() < 1e-6 && (mid[2] - 0.25).abs() < 1e-6);
        assert_eq!(t.sample_ms(500.0), [0.0, 1.0, 0.5]);
    }

    #[test]
    fn rebasing_collapses_the_keys_before_the_band_and_clamps_those_after() {
        let mut keys = vec![(1000u32, 0.0f32), (1133, 60.0)];
        rebase_keys_to_band(&mut keys, 1000, 2600);
        assert_eq!(keys, vec![(0, 0.0), (133, 60.0)]);
        let mut keys = vec![(0u32, 21.4f32)];
        rebase_keys_to_band(&mut keys, 3333, 4333);
        assert_eq!(keys, vec![(0, 21.4)]);
        let mut keys = vec![(0u32, 1.0f32), (500, 2.0), (1200, 3.0), (3000, 4.0)];
        rebase_keys_to_band(&mut keys, 1000, 2000);
        assert_eq!(keys, vec![(0, 2.0), (200, 3.0), (1000, 4.0)]);
        let mut keys = vec![(0u32, 100.0f32), (500, -100.0), (667, -100.0)];
        rebase_keys_to_band(&mut keys, 0, 667);
        assert_eq!(keys, vec![(0, 100.0), (500, -100.0), (667, -100.0)]);
    }

    #[test]
    fn a_band_that_ends_before_it_starts_and_keys_out_of_order_read_without_a_panic() {
        let mut keys = vec![(0u32, 1.0f32), (900, 2.0)];
        rebase_keys_to_band(&mut keys, 800, 500);
        assert_eq!(keys, vec![(0, 1.0), (0, 2.0)]);
        let t = ValueTrack {
            keys: vec![(100, 1.0), (50, 3.0), (200, 5.0)],
            interp: 1,
        };
        assert!(t.sample_ms(120.0).is_finite());
    }
}
