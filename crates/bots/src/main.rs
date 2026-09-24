//! A crowd of bot clients for the server, walking Elwynn's terrain from one process and checking what they are shown.

mod bot;
mod check;
mod ground;
mod mover;
mod region;
mod track;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::bot::{Crowd, Roles, now_ms};
use crate::check::{Counters, PercentilesMs};
use crate::ground::Ground;
use crate::region::Scenario;

const USAGE: &str = "\
usage: bots [--addr HOST:PORT | --in-process] [--scenario goldshire|elwynn] [--count N]
            [--liars N] [--checkers N] [--settle S] [--secs S] [--threads N] [--label TEXT]
         run N bots (100 by default) against a server, each walking the scenario's
         terrain and claiming its movement as the client does; once all are in, wait
         S seconds (10), measure for S seconds (30) and print one row. The first
         --liars bots (1) lie about their speed and teleport; the next --checkers (all)
         check every position they are shown against where its bot really was.
         --in-process runs the server on this process's threads (--server-threads, 1,
         --unchecked to accept every claim) and prints its row too.
       bots spawns [--scenario S] [--count N] [--seed N]
         print N places to join at, one `x y z facing` per line, for the server.
       bots header
         print the header of the row's table.

The terrain is read from the install at $WOW_DATA (or --wow-data DIR).";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("spawns") => spawns(&args[1..]),
        Some("header") => {
            println!("{REPORT_HEADER}");
            Ok(())
        }
        _ => load(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

type Flags = BTreeMap<String, String>;

fn flags(args: &[String], switches: &[&str]) -> Result<Flags, String> {
    let mut out = BTreeMap::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let Some(name) = arg.strip_prefix("--") else {
            return Err(format!("unexpected {arg}"));
        };
        let value = if switches.contains(&name) {
            String::new()
        } else {
            it.next().ok_or(format!("{arg} needs a value"))?.clone()
        };
        out.insert(name.to_string(), value);
    }
    Ok(out)
}

fn num<T: std::str::FromStr>(f: &Flags, name: &str, default: T) -> Result<T, String> {
    f.get(name).map_or(Ok(default), |v| {
        v.parse()
            .map_err(|_| format!("--{name} {v} is not a number"))
    })
}

fn place(f: &Flags) -> Result<(Scenario, Ground), String> {
    let name = f.get("scenario").map_or("goldshire", String::as_str);
    let scenario = region::scenario(name).ok_or(format!("no scenario {name}"))?;
    let data = f
        .get("wow-data")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("WOW_DATA").map(PathBuf::from))
        .ok_or("set WOW_DATA to the install's Data directory")?;
    let chain = mpq::Chain::open(&data).map_err(|e| format!("{}: {e}", data.display()))?;
    let ground = Ground::load(&chain, "Azeroth", scenario.tiles_x, scenario.tiles_y)?;
    Ok((scenario, ground))
}

fn spawns(args: &[String]) -> Result<(), String> {
    let f = flags(args, &[])?;
    let (scenario, ground) = place(&f)?;
    let list = region::spawns(
        &scenario,
        &ground,
        num(&f, "count", 100)?,
        num(&f, "seed", 1)?,
    )?;
    for s in list {
        println!("{} {} {} {}", s.pos[0], s.pos[1], s.pos[2], s.facing);
    }
    Ok(())
}

fn load(args: &[String]) -> Result<(), String> {
    let f = flags(args, &["in-process", "unchecked"])?;
    let (scenario, ground) = place(&f)?;
    let count: usize = num(&f, "count", 100)?;
    let settle: u64 = num(&f, "settle", 10)?;
    let secs: u64 = num(&f, "secs", 30)?;
    let roles = Roles {
        liars: num(&f, "liars", 1)?,
        checkers: num(&f, "checkers", count)?,
    };
    let running = if f.contains_key("in-process") {
        let cfg = server::Config {
            spawns: region::spawns(&scenario, &ground, count, 1)?,
            tick_threads: num(&f, "server-threads", 1)?,
            io_threads: 1,
            rules: server::Rules {
                check: !f.contains_key("unchecked"),
                ..server::Rules::default()
            },
            ..server::Config::default()
        };
        Some(server::start(cfg).map_err(|e| format!("starting the server: {e}"))?)
    } else {
        None
    };
    let addr: SocketAddr = match &running {
        Some(r) => r.addr(),
        None => f
            .get("addr")
            .map_or("127.0.0.1:7777", String::as_str)
            .parse()
            .map_err(|_| "--addr wants HOST:PORT")?,
    };
    let walks_end_ms = now_ms() + ((settle + secs + 120) * 1000) as u32;
    let crowd = Arc::new(Crowd::new(scenario, ground, count * 2, roles, walks_end_ms));
    let mut rt = tokio::runtime::Builder::new_multi_thread();
    if let Ok(n) = num::<usize>(&f, "threads", 0)
        && n > 0
    {
        rt.worker_threads(n);
    }
    let rt = rt
        .enable_all()
        .build()
        .map_err(|e| format!("a runtime: {e}"))?;
    let label = f.get("label").cloned().unwrap_or_default();
    let window = rt.block_on(drive(addr, crowd.clone(), count, settle, secs));
    println!("{}", report(&label, &crowd, count, &window));
    if let Some(running) = running {
        let summary = running.stop().map_err(|e| format!("stopping: {e}"))?;
        println!("{}", summary.row(&label));
    }
    rt.shutdown_background();
    Ok(())
}

struct Measured {
    counters: Counters,
    jitter: PercentilesMs,
    secs: f64,
    cpu_ns: u64,
}

async fn drive(
    addr: SocketAddr,
    crowd: Arc<Crowd>,
    count: usize,
    settle: u64,
    secs: u64,
) -> Measured {
    for i in 0..count {
        tokio::spawn(bot::run(i, addr, crowd.clone()));
        if i % 100 == 99 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    let t = &crowd.traffic;
    let deadline = Instant::now() + Duration::from_secs(60);
    while t.welcomed.load(Ordering::Relaxed) + t.closed.load(Ordering::Relaxed) < count as u64
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(settle)).await;
    let (before, cpu0, t0) = (t.counters(), server::process_cpu_ns(), Instant::now());
    crowd.checks.open.store(true, Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(secs)).await;
    crowd.checks.open.store(false, Ordering::Relaxed);
    let (counters, secs, cpu_ns) = (
        t.counters().since(before),
        t0.elapsed().as_secs_f64(),
        server::process_cpu_ns() - cpu0,
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let jitter = t.jitter.percentiles();
    crowd.stop.store(true, Ordering::Relaxed);
    Measured {
        counters,
        jitter,
        secs,
        cpu_ns,
    }
}

const REPORT_HEADER: &str = "| run | bots in | checkers | batches/s per bot | batch jitter p50 / p99 / max ms | in KB/s per bot | out B/s per bot | claims/s per bot | relayed positions of honest bots checked / off / worst yd; of liars off / worst; matching no claim | view error near: worst yd (bound) / over | middle | far | swept views over bound | missing (deepest yd) / spurious | moves unannounced / appears doubled | lying frames / liar corrections / honest corrections | batch gaps / decode errors | process % of a core | load |";

fn report(label: &str, crowd: &Crowd, count: usize, m: &Measured) -> String {
    let bots = crowd.traffic.welcomed.load(Ordering::Relaxed);
    let per_bot = |v: u64| v as f64 / m.secs / bots.max(1) as f64;
    let c = &crowd.checks;
    let get = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let tiers: Vec<String> = (0..3)
        .map(|t| {
            let j = &c.stale_by_tier[t];
            let bound = crowd.limits.view_lag_bound_yd(t);
            format!("{:.2} ({bound:.2}) / {}", j.worst.get(), get(&j.bad))
        })
        .collect();
    let swept: u64 = c.swept_by_tier.iter().map(|j| get(&j.bad)).sum();
    let n = m.counters;
    let p = &m.jitter;
    format!(
        "| {label} | {bots}/{count} | {} | {:.1} | {} / {} / {} | {:.2} | {:.0} | {:.2} | {} / {} / {:.4}; {} / {:.3}; {} | {} | {} | {} ({:.1}) / {} | {} / {} | {} / {} / {} | {} / {} | {:.1} | {} |",
        crowd.roles.checkers.min(count),
        per_bot(n.batches),
        p.p50,
        p.p99,
        p.max,
        per_bot(n.bytes_in) / 1e3,
        per_bot(n.bytes_out),
        per_bot(n.claims),
        get(&c.relayed_honest.checked),
        get(&c.relayed_honest.bad),
        c.relayed_honest.worst.get(),
        get(&c.relayed_liars.bad),
        c.relayed_liars.worst.get(),
        get(&c.relayed_unclaimed),
        tiers.join(" | "),
        swept,
        get(&c.missing),
        c.missing_depth_yd.get(),
        get(&c.spurious),
        get(&c.unknown_moves),
        get(&c.double_appears),
        get(&c.lying_frames),
        get(&c.liar_corrections),
        get(&c.honest_corrections),
        n.gaps,
        n.decode_errors,
        m.cpu_ns as f64 / 1e9 / m.secs * 100.0,
        server::load_average(),
    )
}
