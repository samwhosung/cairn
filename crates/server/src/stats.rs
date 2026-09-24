use std::sync::atomic::{AtomicU64, Ordering};

use rustix::time::{ClockId, clock_gettime};

/// CPU time this thread has run, ns: other processes preempting it do not count.
pub fn thread_cpu_ns() -> u64 {
    let t = clock_gettime(ClockId::ThreadCPUTime);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// CPU time the whole process has run, ns.
pub fn process_cpu_ns() -> u64 {
    let t = clock_gettime(ClockId::ProcessCPUTime);
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

/// One phase of a tick's CPU, summed over the threads that ran it, and its largest single task.
#[derive(Default)]
pub struct Phase {
    cpu: AtomicU64,
    largest: AtomicU64,
}

impl Phase {
    /// Runs `f` on this thread and counts the CPU it took as one task of this phase.
    pub fn time<R>(&self, f: impl FnOnce() -> R) -> R {
        let started = thread_cpu_ns();
        let out = f();
        let ns = thread_cpu_ns().saturating_sub(started);
        self.cpu.fetch_add(ns, Ordering::Relaxed);
        self.largest.fetch_max(ns, Ordering::Relaxed);
        out
    }

    /// The CPU and the largest task since the last take.
    pub fn take(&self) -> (u64, u64) {
        (
            self.cpu.swap(0, Ordering::Relaxed),
            self.largest.swap(0, Ordering::Relaxed),
        )
    }
}

/// The phases of a tick, in order.
pub const PHASES: [&str; 5] = ["admit", "step", "index", "replicate", "hash"];

/// What one tick cost and did.
#[derive(Clone, Copy, Debug, Default)]
pub struct TickStats {
    pub tick: u32,
    pub players: u32,
    pub cpu: [u64; 5],
    pub largest: [u64; 5],
    pub wall: [u64; 5],
    pub claims: u32,
    pub refused: u32,
    pub stale: u32,
    pub appeared: u32,
    pub vanished: u32,
    pub moves: u32,
    pub deferred: u32,
    pub corrections: u32,
    pub kicked: u32,
    pub bytes_out: u64,
    pub hash: u64,
}

impl TickStats {
    /// The tick an idle machine with `threads` cores would take, ns: each phase's CPU spread
    /// over the threads, or its largest task when that is longer.
    pub fn ideal(&self, threads: usize) -> u64 {
        (0..PHASES.len())
            .map(|p| (self.cpu[p] / threads.max(1) as u64).max(self.largest[p]))
            .sum()
    }

    pub fn cpu_total(&self) -> u64 {
        self.cpu.iter().sum()
    }

    pub fn wall_total(&self) -> u64 {
        self.wall.iter().sum()
    }
}

/// A run's ticks over its measured window, summarised.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub threads: usize,
    pub ticks: usize,
    pub players: u32,
    /// Percentiles 50, 99 and 100 of the ideal tick, ms.
    pub ideal: [f64; 3],
    /// …of the CPU a tick took over all threads, ms.
    pub cpu: [f64; 3],
    /// …of the tick's wall time, ms.
    pub wall: [f64; 3],
    /// Mean CPU per tick by phase, ms.
    pub phase_cpu: [f64; 5],
    pub out_per_client: f64,
    pub in_per_client: f64,
    pub out_total: f64,
    pub claims_per_client: f64,
    pub refused: u64,
    pub stale: u64,
    pub deferred: u64,
    pub kicked: u64,
    pub moves_per_client: f64,
    pub hash: u64,
    /// The whole process's CPU over the window as a share of one core.
    pub process_share: f64,
}

impl Summary {
    /// Summarises `ticks`, which ran `secs` of real time while `bytes_in` arrived and the
    /// process spent `process_ns` of CPU.
    pub fn of(
        ticks: &[TickStats],
        threads: usize,
        secs: f64,
        bytes_in: u64,
        process_ns: u64,
    ) -> Self {
        let n = ticks.len().max(1) as f64;
        let players = ticks.iter().map(|t| t.players).max().unwrap_or(0);
        let per_client_s = f64::from(players.max(1)) * secs.max(1e-9);
        let sum = |f: &dyn Fn(&TickStats) -> u64| ticks.iter().map(f).sum::<u64>();
        let ms = |f: &dyn Fn(&TickStats) -> u64| percentiles(ticks.iter().map(f).collect());
        let mut phase_cpu = [0.0; 5];
        for (p, v) in phase_cpu.iter_mut().enumerate() {
            *v = sum(&|t| t.cpu[p]) as f64 / n / 1e6;
        }
        Self {
            threads,
            ticks: ticks.len(),
            players,
            ideal: ms(&|t| t.ideal(threads)),
            cpu: ms(&TickStats::cpu_total),
            wall: ms(&TickStats::wall_total),
            phase_cpu,
            out_per_client: sum(&|t| t.bytes_out) as f64 / per_client_s,
            in_per_client: bytes_in as f64 / per_client_s,
            out_total: sum(&|t| t.bytes_out) as f64 / secs.max(1e-9),
            claims_per_client: sum(&|t| u64::from(t.claims)) as f64 / per_client_s,
            refused: sum(&|t| u64::from(t.refused)),
            stale: sum(&|t| u64::from(t.stale)),
            deferred: sum(&|t| u64::from(t.deferred)),
            kicked: sum(&|t| u64::from(t.kicked)),
            moves_per_client: sum(&|t| u64::from(t.moves)) as f64 / per_client_s,
            hash: ticks.last().map_or(0, |t| t.hash),
            process_share: process_ns as f64 / 1e9 / secs.max(1e-9),
        }
    }
}

impl Summary {
    /// The header of [`Summary::row`]'s table.
    pub const HEADER: &str = "| run | players | threads | ticks | ideal p50 ms | ideal p99 | ideal max | CPU p50 ms | CPU p99 | wall p50 ms | wall p99 | CPU by phase: admit / step / index / replicate / hash, ms | out KB/s per client | in B/s per client | out MB/s | claims/s per client | moves/s per client | refused | stale | deferred | kicked | process % of a core | world hash | load |";

    /// One markdown row: the tick's cost, the bytes, what was refused and shed, and the load
    /// average when it was printed.
    pub fn row(&self, label: &str) -> String {
        let [i50, i99, imax] = self.ideal;
        let [c50, c99, _] = self.cpu;
        let [w50, w99, _] = self.wall;
        let phases: Vec<String> = self.phase_cpu.iter().map(|p| format!("{p:.3}")).collect();
        format!(
            "| {label} | {} | {} | {} | {i50:.3} | {i99:.3} | {imax:.3} | {c50:.3} | {c99:.3} | {w50:.3} | {w99:.3} | {} | {:.2} | {:.0} | {:.2} | {:.1} | {:.1} | {} | {} | {} | {} | {:.1} | {:016x} | {} |",
            self.players,
            self.threads,
            self.ticks,
            phases.join(" / "),
            self.out_per_client / 1e3,
            self.in_per_client,
            self.out_total / 1e6,
            self.claims_per_client,
            self.moves_per_client,
            self.refused,
            self.stale,
            self.deferred,
            self.kicked,
            self.process_share * 100.0,
            self.hash,
            load_average(),
        )
    }
}

/// The 1-, 5- and 15-minute load averages, as the system reports them.
pub fn load_average() -> String {
    let text = std::fs::read_to_string("/proc/loadavg").ok().or_else(|| {
        std::process::Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
    });
    text.map(|t| {
        t.split_whitespace()
            .filter(|w| w.parse::<f64>().is_ok())
            .take(3)
            .collect::<Vec<_>>()
            .join(" ")
    })
    .unwrap_or_default()
}

/// Percentiles 50, 99 and 100 of `ns`, in ms.
fn percentiles(mut ns: Vec<u64>) -> [f64; 3] {
    if ns.is_empty() {
        return [0.0; 3];
    }
    ns.sort_unstable();
    let at = |q: f64| ns[((ns.len() - 1) as f64 * q).round() as usize] as f64 / 1e6;
    [at(0.5), at(0.99), at(1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ideal_tick_is_spread_cpu_or_the_largest_task() {
        let t = TickStats {
            cpu: [100, 1400, 10, 2800, 0],
            largest: [100, 50, 10, 400, 0],
            ..TickStats::default()
        };
        assert_eq!(t.ideal(14), 100 + 100 + 10 + 400);
        assert_eq!(t.ideal(1), 4310);
    }

    #[test]
    fn a_thread_clock_counts_work_and_not_sleep() {
        let phase = Phase::default();
        phase.time(|| std::thread::sleep(std::time::Duration::from_millis(30)));
        let (slept, _) = phase.take();
        assert!(slept < 10_000_000, "{slept} ns");
        let spun = phase.time(|| (0..200_000u64).fold(0u64, |a, b| a.wrapping_add(b * b)));
        std::hint::black_box(spun);
        let (cpu, largest) = phase.take();
        assert!(cpu > 0 && largest == cpu, "{cpu} {largest}");
    }
}
