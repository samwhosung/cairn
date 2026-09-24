use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::mover::HEARTBEAT_MS;
use crate::track::RUN;

pub const CLAIM_TO_BATCH_MS: f32 = 150.0;
pub const RELAYED_EPSILON_YD: f32 = 0.01;

pub struct Limits {
    tiers: [server::Tier; 3],
    tick_ms: f32,
    pub view_yd: f32,
    pub presence_slack_yd: f32,
    tier_slack_yd: f32,
}

impl Limits {
    pub fn of_server() -> Self {
        let view = server::View::default();
        let tick_ms = f32::from(server::Config::default().tick_ms);
        let both_running = 2.0 * RUN / 1000.0;
        let heartbeat_and_pipe = HEARTBEAT_MS as f32 + CLAIM_TO_BATCH_MS;
        let recheck_ms = view.aoi_every as f32 * tick_ms;
        Self {
            tiers: view.tiers,
            tick_ms,
            view_yd: view.radius + view.grey,
            presence_slack_yd: both_running * (recheck_ms + heartbeat_and_pipe),
            tier_slack_yd: both_running * heartbeat_and_pipe,
        }
    }

    pub fn tier(&self, yd: f32) -> usize {
        self.tiers
            .iter()
            .position(|t| yd + self.tier_slack_yd <= t.within)
            .unwrap_or(self.tiers.len() - 1)
    }

    pub fn view_lag_bound_yd(&self, tier: usize) -> f32 {
        let period = self.tiers[tier].every as f32 * self.tick_ms;
        RUN * (HEARTBEAT_MS as f32 + period + CLAIM_TO_BATCH_MS) / 1000.0
    }
}

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

#[derive(Default)]
pub struct Judged {
    pub checked: AtomicU64,
    pub bad: AtomicU64,
    pub worst: Worst,
}

impl Judged {
    pub fn judge(&self, value: f32, bound: f32) {
        let mut one = UnsentJudged::default();
        one.judge(value, bound);
        self.absorb(&mut one);
    }

    pub fn absorb(&self, unsent: &mut UnsentJudged) {
        if unsent.checked > 0 {
            self.checked.fetch_add(unsent.checked, Ordering::Relaxed);
            self.bad.fetch_add(unsent.bad, Ordering::Relaxed);
            self.worst.note(unsent.worst);
        }
        *unsent = UnsentJudged::default();
    }
}

#[derive(Default)]
pub struct UnsentJudged {
    checked: u64,
    bad: u64,
    worst: f32,
}

impl UnsentJudged {
    pub fn judge(&mut self, value: f32, bound: f32) {
        self.checked += 1;
        self.worst = self.worst.max(value);
        if value > bound {
            self.bad += 1;
        }
    }
}

#[derive(Default)]
pub struct Checks {
    pub open: AtomicBool,
    pub relayed_honest: Judged,
    pub relayed_liars: Judged,
    pub stale_by_tier: [Judged; 3],
    pub swept_by_tier: [Judged; 3],
    pub missing: AtomicU64,
    pub missing_depth_yd: Worst,
    pub spurious: AtomicU64,
    pub unknown_moves: AtomicU64,
    pub double_appears: AtomicU64,
    pub lying_frames: AtomicU64,
    pub liar_corrections: AtomicU64,
    pub honest_corrections: AtomicU64,
}

impl Checks {
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    pub fn count(n: &AtomicU64) {
        n.fetch_add(1, Ordering::Relaxed);
    }

    pub fn absorb(n: &AtomicU64, unsent: &mut u64) {
        if *unsent > 0 {
            n.fetch_add(*unsent, Ordering::Relaxed);
            *unsent = 0;
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub batches: u64,
    pub claims: u64,
    pub gaps: u64,
    pub decode_errors: u64,
}

impl Counters {
    pub fn since(self, before: Self) -> Self {
        Self {
            bytes_in: self.bytes_in - before.bytes_in,
            bytes_out: self.bytes_out - before.bytes_out,
            batches: self.batches - before.batches,
            claims: self.claims - before.claims,
            gaps: self.gaps - before.gaps,
            decode_errors: self.decode_errors - before.decode_errors,
        }
    }
}

#[derive(Default)]
pub struct Traffic {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
    pub batches: AtomicU64,
    pub claims: AtomicU64,
    pub gaps: AtomicU64,
    pub decode_errors: AtomicU64,
    pub welcomed: AtomicU64,
    pub closed: AtomicU64,
    pub jitter: JitterHistogram,
}

impl Traffic {
    pub fn counters(&self) -> Counters {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        Counters {
            bytes_in: get(&self.bytes_in),
            bytes_out: get(&self.bytes_out),
            batches: get(&self.batches),
            claims: get(&self.claims),
            gaps: get(&self.gaps),
            decode_errors: get(&self.decode_errors),
        }
    }
}

pub struct JitterHistogram {
    buckets: Box<[AtomicU64]>,
}

pub struct PercentilesMs {
    pub p50: u32,
    pub p99: u32,
    pub max: u32,
}

impl Default for JitterHistogram {
    fn default() -> Self {
        Self {
            buckets: (0..Self::BUCKETS).map(|_| AtomicU64::new(0)).collect(),
        }
    }
}

impl JitterHistogram {
    const BUCKET_MS: u32 = 5;
    const BUCKETS: usize = 2000;

    pub fn add(&self, ms: u32) {
        let b = ((ms / Self::BUCKET_MS) as usize).min(Self::BUCKETS - 1);
        self.buckets[b].fetch_add(1, Ordering::Relaxed);
    }

    pub fn percentiles(&self) -> PercentilesMs {
        let counts: Vec<u64> = self
            .buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect();
        let total: u64 = counts.iter().sum();
        let at = |q: f64| {
            let want = ((total as f64) * q).ceil().max(1.0) as u64;
            let mut seen = 0;
            counts
                .iter()
                .position(|&n| {
                    seen += n;
                    seen >= want
                })
                .map_or(0, |b| b as u32 * Self::BUCKET_MS)
        };
        PercentilesMs {
            p50: at(0.5),
            p99: at(0.99),
            max: at(1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_are_judged_loosely_and_bounds_grow_with_them() {
        let limits = Limits::of_server();
        assert_eq!(limits.tier(5.0), 0);
        assert_eq!(limits.tier(20.0), 1);
        assert_eq!(limits.tier(45.0), 2);
        let bound = |t| limits.view_lag_bound_yd(t);
        assert!(bound(0) < bound(1) && bound(1) < bound(2));
        assert!((bound(0) - 4.9).abs() < 1e-4);
    }

    #[test]
    fn percentiles_read_the_buckets() {
        let jitter = JitterHistogram::default();
        for _ in 0..98 {
            jitter.add(7);
        }
        jitter.add(21);
        jitter.add(48);
        let p = jitter.percentiles();
        assert_eq!((p.p50, p.p99, p.max), (5, 20, 45));
    }
}
