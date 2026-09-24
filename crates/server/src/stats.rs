use std::sync::atomic::{AtomicU64, Ordering};

use crate::replicate::Built;
use crate::rules::Why;

pub use clock::{process_cpu_ns, thread_cpu_ns};

#[cfg(unix)]
mod clock {
    use rustix::time::{ClockId, clock_gettime};

    pub fn thread_cpu_ns() -> u64 {
        let t = clock_gettime(ClockId::ThreadCPUTime);
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }

    pub fn process_cpu_ns() -> u64 {
        let t = clock_gettime(ClockId::ProcessCPUTime);
        t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
    }
}

/// Windows counts CPU time in scheduler ticks, 15.6 ms by default, so there a short task often
/// reads as none.
#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "the thread and process CPU times are C APIs with no safe binding in the tree"
)]
mod clock {
    use core::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> *mut c_void;
        fn GetCurrentProcess() -> *mut c_void;
        fn GetThreadTimes(
            thread: *mut c_void,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            created: *mut u64,
            exited: *mut u64,
            kernel: *mut u64,
            user: *mut u64,
        ) -> i32;
    }

    pub fn thread_cpu_ns() -> u64 {
        let [mut created, mut exited, mut kernel, mut user] = [0u64; 4];
        // SAFETY: the calling thread's pseudo-handle, and four out-parameters, each a FILETIME:
        // two u32s, low first, which is a little-endian u64 of 100 ns.
        unsafe {
            GetThreadTimes(
                GetCurrentThread(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            );
        }
        (kernel + user) * 100
    }

    pub fn process_cpu_ns() -> u64 {
        let [mut created, mut exited, mut kernel, mut user] = [0u64; 4];
        // SAFETY: as in `thread_cpu_ns`, with the calling process's pseudo-handle.
        unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            );
        }
        (kernel + user) * 100
    }
}

#[derive(Default)]
pub struct Phase {
    cpu_ns: AtomicU64,
    largest_task_ns: AtomicU64,
}

pub struct PhaseTaken {
    pub cpu_ns: u64,
    pub largest_task_ns: u64,
}

impl Phase {
    pub fn time<R>(&self, f: impl FnOnce() -> R) -> R {
        let started = thread_cpu_ns();
        let out = f();
        let ns = thread_cpu_ns().saturating_sub(started);
        self.cpu_ns.fetch_add(ns, Ordering::Relaxed);
        self.largest_task_ns.fetch_max(ns, Ordering::Relaxed);
        out
    }

    pub fn take(&self) -> PhaseTaken {
        PhaseTaken {
            cpu_ns: self.cpu_ns.swap(0, Ordering::Relaxed),
            largest_task_ns: self.largest_task_ns.swap(0, Ordering::Relaxed),
        }
    }
}

/// The phases of a tick in the order they run, which is also the order of every per-phase array.
pub const PHASES: [&str; 6] = ["admit", "step", "index", "encode", "replicate", "hash"];

#[derive(Clone, Copy, Debug, Default)]
pub struct TickStats {
    pub tick: u32,
    pub players: u32,
    pub cpu_ns: [u64; PHASES.len()],
    pub largest_task_ns: [u64; PHASES.len()],
    pub wall_ns: [u64; PHASES.len()],
    pub claims: u32,
    pub refused: [u32; Why::ALL.len()],
    pub stale: u32,
    pub built: Built,
    pub hash: u64,
}

impl TickStats {
    pub fn ideal_ns(&self, threads: usize) -> u64 {
        (0..PHASES.len())
            .map(|p| (self.cpu_ns[p] / threads.max(1) as u64).max(self.largest_task_ns[p]))
            .sum()
    }

    pub fn cpu_total_ns(&self) -> u64 {
        self.cpu_ns.iter().sum()
    }

    pub fn wall_total_ns(&self) -> u64 {
        self.wall_ns.iter().sum()
    }
}

/// A run's ticks over its measured window.
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub threads: usize,
    pub ticks: usize,
    pub players: u32,
    /// Percentiles 50, 99 and 100 of the ideal tick: the tick an idle machine with `threads`
    /// cores would take, each phase its CPU spread over them or its largest task, ms.
    pub ideal: [f64; 3],
    /// Percentiles 50, 99 and 100 of the CPU a tick took over all threads, ms.
    pub cpu: [f64; 3],
    /// Percentiles 50, 99 and 100 of the tick's wall time, ms.
    pub wall: [f64; 3],
    /// Mean CPU per tick of each of [`PHASES`], ms.
    pub phase_cpu: [f64; PHASES.len()],
    pub out_per_client: f64,
    pub in_per_client: f64,
    pub out_total: f64,
    pub claims_per_client: f64,
    /// Refusals by each of [`Why::ALL`].
    pub refused: [u64; Why::ALL.len()],
    pub stale: u64,
    pub deferred: u64,
    pub kicked: u64,
    pub appears_without_slot: u64,
    pub movements_per_client: f64,
    pub turns_per_client: f64,
    pub states_per_client: f64,
    /// Moves and turns a client received per second by tier, nearest first.
    pub tiers_per_client: [f64; 3],
    pub bytes_per_movement: f64,
    /// The fraction of the bytes sent that were copied from pieces encoded once for everyone.
    pub shared: f64,
    pub hash: u64,
    /// The whole process's CPU over the window as a share of one core.
    pub process_share: f64,
    /// Ticks from the window's end to the server's stop; 0 when the window never ended.
    pub ticks_after: u32,
    /// Players still in when the grace ran out, whose connections the server dropped.
    pub stayed: u32,
}

impl Summary {
    pub fn of(
        ticks: &[TickStats],
        threads: usize,
        wall_secs: f64,
        bytes_in: u64,
        process_ns: u64,
    ) -> Self {
        let n = ticks.len().max(1) as f64;
        let players = ticks.iter().map(|t| t.players).max().unwrap_or(0);
        let per_client_s = f64::from(players.max(1)) * wall_secs.max(1e-9);
        let sum = |f: &dyn Fn(&TickStats) -> u64| ticks.iter().map(f).sum::<u64>();
        let ms = |f: &dyn Fn(&TickStats) -> u64| p50_p99_max_ms(ticks.iter().map(f).collect());
        let mut phase_cpu = [0.0; PHASES.len()];
        for (p, v) in phase_cpu.iter_mut().enumerate() {
            *v = sum(&|t| t.cpu_ns[p]) as f64 / n / 1e6;
        }
        let built = ticks.iter().fold(Built::default(), |a, t| a.add(t.built));
        let bytes_out = built.bytes as f64;
        let movements = f64::from(built.moves + built.turns + built.states);
        Self {
            threads,
            ticks: ticks.len(),
            players,
            ideal: ms(&|t| t.ideal_ns(threads)),
            cpu: ms(&TickStats::cpu_total_ns),
            wall: ms(&TickStats::wall_total_ns),
            phase_cpu,
            out_per_client: bytes_out / per_client_s,
            in_per_client: bytes_in as f64 / per_client_s,
            out_total: bytes_out / wall_secs.max(1e-9),
            claims_per_client: sum(&|t| u64::from(t.claims)) as f64 / per_client_s,
            refused: std::array::from_fn(|i| sum(&|t| u64::from(t.refused[i]))),
            stale: sum(&|t| u64::from(t.stale)),
            deferred: sum(&|t| u64::from(t.built.deferred)),
            kicked: sum(&|t| u64::from(t.built.kicked)),
            appears_without_slot: sum(&|t| u64::from(t.built.appears_without_slot)),
            movements_per_client: movements / per_client_s,
            turns_per_client: f64::from(built.turns) / per_client_s,
            states_per_client: f64::from(built.states) / per_client_s,
            tiers_per_client: built
                .moves_and_turns_by_tier
                .map(|n| f64::from(n) / per_client_s),
            bytes_per_movement: built.movement_bytes as f64 / movements.max(1.0),
            shared: built.shared_bytes as f64 / bytes_out.max(1.0),
            hash: ticks.last().map_or(0, |t| t.hash),
            process_share: process_ns as f64 / 1e9 / wall_secs.max(1e-9),
            ticks_after: 0,
            stayed: 0,
        }
    }

    pub fn header() -> String {
        let why: Vec<String> = Why::ALL
            .iter()
            .map(|w| format!("{w:?}").to_lowercase())
            .collect();
        format!(
            "| run | players | threads | ticks | ideal p50 ms | ideal p99 | ideal max | CPU p50 ms | CPU p99 | wall p50 ms | wall p99 | CPU by phase: {}, ms | out KB/s per client | in B/s per client | out MB/s | claims/s per client | movements/s per client | of them turns / states | moves and turns by tier, near / middle / far | B per movement | shared % of bytes out | refused: {} | stale | deferred | kicked / appears without a slot | process % of a core | world hash | load |",
            PHASES.join(" / "),
            why.join(" / ")
        )
    }

    /// One markdown row of [`Summary::header`]'s table, with the load average when it is made.
    pub fn row(&self, label: &str) -> String {
        let [i50, i99, imax] = self.ideal;
        let [c50, c99, _] = self.cpu;
        let [w50, w99, _] = self.wall;
        let phases: Vec<String> = self.phase_cpu.iter().map(|p| format!("{p:.3}")).collect();
        format!(
            "| {label} | {} | {} | {} | {i50:.3} | {i99:.3} | {imax:.3} | {c50:.3} | {c99:.3} | {w50:.3} | {w99:.3} | {} | {:.2} | {:.0} | {:.2} | {:.1} | {:.1} | {:.1} / {:.1} | {} | {:.2} | {:.1} | {} | {} | {} | {} / {} | {:.1} | {:016x} | {} |",
            self.players,
            self.threads,
            self.ticks,
            phases.join(" / "),
            self.out_per_client / 1e3,
            self.in_per_client,
            self.out_total / 1e6,
            self.claims_per_client,
            self.movements_per_client,
            self.turns_per_client,
            self.states_per_client,
            self.tiers_per_client.map(|n| format!("{n:.1}")).join(" / "),
            self.bytes_per_movement,
            self.shared * 100.0,
            self.refused.map(|n| n.to_string()).join(" / "),
            self.stale,
            self.deferred,
            self.kicked,
            self.appears_without_slot,
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

fn p50_p99_max_ms(mut ns: Vec<u64>) -> [f64; 3] {
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
            cpu_ns: [100, 1400, 10, 70, 2800, 0],
            largest_task_ns: [100, 50, 10, 20, 400, 0],
            ..TickStats::default()
        };
        assert_eq!(t.ideal_ns(14), 100 + 100 + 10 + 20 + 400);
        assert_eq!(t.ideal_ns(1), 4380);
    }

    #[test]
    fn a_thread_clock_counts_work_and_not_sleep() {
        let phase = Phase::default();
        phase.time(|| std::thread::sleep(std::time::Duration::from_millis(30)));
        let slept = phase.take().cpu_ns;
        assert!(slept < 10_000_000, "{slept} ns");
        let spun = phase.time(|| {
            let started = std::time::Instant::now();
            let mut n = 0u64;
            while started.elapsed() < std::time::Duration::from_millis(50) {
                n = std::hint::black_box(n.wrapping_add(1));
            }
            n
        });
        std::hint::black_box(spun);
        let taken = phase.take();
        assert!(taken.cpu_ns > 0 && taken.largest_task_ns == taken.cpu_ns);
    }
}
