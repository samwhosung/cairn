use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::mover::HEARTBEAT_MS;
use crate::track::RUN;

/// The server's refresh tiers: within this many yards, one refresh per this many ticks.
pub const TIERS: [(f32, u32); 3] = [(25.0, 1), (50.0, 4), (f32::INFINITY, 10)];
pub const TICK_MS: f32 = 50.0;
/// Allowed for a claim to wait for its tick, the tick to run, and the batch to arrive, ms.
pub const PIPE_MS: f32 = 150.0;
/// A tier is judged by the observer's distance plus this, since both sides move between the
/// server's measure and ours.
pub const TIER_SLACK_YD: f32 = 10.0;
pub const VIEW_YD: f32 = 101.0;
/// Presence is judged only this far inside or outside the view distance: the server rechecks
/// who is in view every 250 ms, from positions up to a heartbeat old, while both sides move.
pub const PRESENCE_SLACK_YD: f32 = 15.0;
/// A relayed position further than this from where its bot was at the claim's time is a lie
/// or a corruption, yards.
pub const EXACT_YD: f32 = 0.01;

/// The tier a bot at `yd` yards falls in, judged loosely.
pub fn tier(yd: f32) -> usize {
    TIERS
        .iter()
        .position(|&(within, _)| yd + TIER_SLACK_YD <= within)
        .unwrap_or(TIERS.len() - 1)
}

/// How far behind its bot a view may be in `tier`, yards: a run for a heartbeat, the tier's
/// refresh period, and the pipe.
pub fn bound_yd(tier: usize) -> f32 {
    let period = TIERS[tier].1 as f32 * TICK_MS;
    RUN * (HEARTBEAT_MS as f32 + period + PIPE_MS) / 1000.0
}

/// A running maximum of a non-negative float.
#[derive(Default)]
pub struct Worst(AtomicU32);

impl Worst {
    pub fn note(&self, v: f32) {
        self.0.fetch_max(v.max(0.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// A count and how many of those counted broke their bound, with the worst seen.
#[derive(Default)]
pub struct Judged {
    pub checked: AtomicU64,
    pub bad: AtomicU64,
    pub worst: Worst,
}

impl Judged {
    pub fn judge(&self, value: f32, bound: f32) {
        self.checked.fetch_add(1, Ordering::Relaxed);
        self.worst.note(value);
        if value > bound {
            self.bad.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Everything the checking bots found while the window was open.
#[derive(Default)]
pub struct Checks {
    pub open: AtomicBool,
    /// Relayed positions against where their bot was at the claim's time.
    pub exact: Judged,
    /// A view's error the moment a refresh replaced it, by tier.
    pub stale: [Judged; 3],
    /// Every view's error at a sweep, by tier.
    pub swept: [Judged; 3],
    pub missing: AtomicU64,
    /// How far inside the view distance a missing bot stood, at worst, yards.
    pub missing_depth: Worst,
    pub spurious: AtomicU64,
    pub unknown_moves: AtomicU64,
    pub double_appears: AtomicU64,
    /// Frames in which a liar lied, however many claims each carried.
    pub lies: AtomicU64,
    pub liar_corrections: AtomicU64,
    pub honest_corrections: AtomicU64,
}

impl Checks {
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    pub fn missing(&self, yd: f32) {
        self.missing.fetch_add(1, Ordering::Relaxed);
        self.missing_depth.note(VIEW_YD - yd);
    }

    pub fn count(n: &AtomicU64) {
        n.fetch_add(1, Ordering::Relaxed);
    }
}

/// Byte and message counts over every bot, and the lag of batches behind the tick clock.
pub struct Traffic {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub batches: AtomicU64,
    pub records: AtomicU64,
    pub claims: AtomicU64,
    pub gaps: AtomicU64,
    pub decode_errors: AtomicU64,
    pub welcomed: AtomicU64,
    pub closed: AtomicU64,
    /// Batches by how late they arrived against the tick clock, in 5 ms buckets.
    pub lag: Box<[AtomicU64]>,
}

pub const LAG_BUCKETS: usize = 2000;
pub const LAG_BUCKET_MS: u32 = 5;

impl Default for Traffic {
    fn default() -> Self {
        Self {
            bytes_in: AtomicU64::new(0),
            bytes_out: AtomicU64::new(0),
            batches: AtomicU64::new(0),
            records: AtomicU64::new(0),
            claims: AtomicU64::new(0),
            gaps: AtomicU64::new(0),
            decode_errors: AtomicU64::new(0),
            welcomed: AtomicU64::new(0),
            closed: AtomicU64::new(0),
            lag: (0..LAG_BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl Traffic {
    pub fn lag(&self, ms: u32) {
        let b = ((ms / LAG_BUCKET_MS) as usize).min(LAG_BUCKETS - 1);
        self.lag[b].fetch_add(1, Ordering::Relaxed);
    }

    /// Percentiles 50, 99 and 100 of the lag, ms, over the counts in `lag`.
    pub fn lag_percentiles(lag: &[u64]) -> [u32; 3] {
        let total: u64 = lag.iter().sum();
        let at = |q: f64| {
            let want = ((total as f64) * q).ceil().max(1.0) as u64;
            let mut seen = 0;
            lag.iter()
                .position(|&n| {
                    seen += n;
                    seen >= want
                })
                .map_or(0, |b| b as u32 * LAG_BUCKET_MS)
        };
        [at(0.5), at(0.99), at(1.0)]
    }

    /// A copy of every counter, to difference two moments.
    pub fn snapshot(&self) -> Vec<u64> {
        [
            &self.bytes_in,
            &self.bytes_out,
            &self.batches,
            &self.records,
            &self.claims,
            &self.gaps,
            &self.decode_errors,
        ]
        .into_iter()
        .chain(self.lag.iter())
        .map(|c| c.load(Ordering::Relaxed))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_judged_loosely_and_bounds_grow_with_them() {
        assert_eq!(tier(5.0), 0);
        assert_eq!(tier(20.0), 1);
        assert_eq!(tier(45.0), 2);
        assert!(bound_yd(0) < bound_yd(1) && bound_yd(1) < bound_yd(2));
        assert!((bound_yd(0) - 4.9).abs() < 1e-4);
    }

    #[test]
    fn lag_percentiles_read_the_buckets() {
        let mut lag = vec![0u64; 10];
        lag[1] = 98;
        lag[4] = 1;
        lag[9] = 1;
        assert_eq!(Traffic::lag_percentiles(&lag), [5, 20, 45]);
    }
}
