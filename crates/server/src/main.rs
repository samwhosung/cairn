//! Serves one world over TCP until its measured window ends, or replays a recorded run.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use server::{Config, Order, Rules, Spawn, Summary, Window};

const USAGE: &str = "\
usage: server [--port P] [--threads N] [--io-threads N] [--spawns FILE] [--unchecked]
              [--record FILE] [--players N --settle S --measure S] [--label TEXT]
         serve a world on 127.0.0.1:P (7777 by default). With --players, once N players
         are in, wait S seconds, measure for S seconds, print one summary row and stop.
         --unchecked accepts every well-formed claim. --record writes every tick's inputs
         and world hash to FILE.
       server header
         print the header of the summary row's table.
       server replay FILE [--threads N] [--racy]
         replay a recorded run and compare the world after every tick; --racy applies
         each entity's inputs in the order worker threads hand them over.

FILE of spawns: one `x y z facing` per line, WoW world coordinates and radians.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("replay") => replay(&args[1..]),
        Some("header") => {
            println!("{}", Summary::HEADER);
            Ok(())
        }
        _ => serve(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// `--flag value` pairs and bare `--switch`es.
fn flags(args: &[String], switches: &[&str]) -> Result<BTreeMap<String, String>, String> {
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

fn num<T: std::str::FromStr>(
    f: &BTreeMap<String, String>,
    name: &str,
    default: T,
) -> Result<T, String> {
    f.get(name).map_or(Ok(default), |v| {
        v.parse()
            .map_err(|_| format!("--{name} {v} is not a number"))
    })
}

fn serve(args: &[String]) -> Result<(), String> {
    let f = flags(args, &["unchecked"])?;
    let defaults = Config::default();
    let tick_ms = defaults.tick_ms;
    let ticks = |secs: f64| (secs * 1000.0 / f64::from(tick_ms)).round() as u32;
    let window = match f.get("players") {
        Some(_) => Some(Window {
            players: num(&f, "players", 0)?,
            settle: ticks(num(&f, "settle", 5.0)?),
            measure: ticks(num(&f, "measure", 20.0)?),
        }),
        None => None,
    };
    let spawns = match f.get("spawns") {
        Some(path) => read_spawns(Path::new(path))?,
        None => Vec::new(),
    };
    let cfg = Config {
        addr: SocketAddr::from(([127, 0, 0, 1], num(&f, "port", 7777)?)),
        threads: num(&f, "threads", defaults.threads)?,
        io_threads: num(&f, "io-threads", defaults.io_threads)?,
        spawns,
        rules: Rules {
            check: !f.contains_key("unchecked"),
            ..Rules::default()
        },
        record: f.get("record").map(PathBuf::from),
        window,
        ..defaults
    };
    let running = server::start(cfg).map_err(|e| format!("starting: {e}"))?;
    eprintln!("serving on {}", running.addr());
    let summary = running.wait().map_err(|e| format!("serving: {e}"))?;
    let label = f.get("label").map_or("", String::as_str);
    println!("{}", summary.row(label));
    Ok(())
}

fn replay(args: &[String]) -> Result<(), String> {
    let (path, rest) = args.split_first().ok_or("replay needs a FILE")?;
    let f = flags(rest, &["racy"])?;
    let threads = num(&f, "threads", 1)?;
    let order = if f.contains_key("racy") {
        Order::Racy
    } else {
        Order::Canonical
    };
    let r = server::replay(Path::new(path), threads, order).map_err(|e| format!("{path}: {e}"))?;
    let verdict = r
        .first_mismatch
        .map_or("every tick matches".to_string(), |t| {
            format!("differs from tick {t}")
        });
    println!(
        "replay threads={threads} order={order:?} ticks={} hash={:016x} {verdict}",
        r.ticks, r.hash
    );
    Ok(())
}

fn read_spawns(path: &Path) -> Result<Vec<Spawn>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut spawns = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let v: Vec<f32> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|_| format!("{}:{}: not numbers", path.display(), n + 1))?;
        let [x, y, z, facing] = v[..] else {
            return Err(format!("{}:{}: want x y z facing", path.display(), n + 1));
        };
        spawns.push(Spawn {
            pos: [x, y, z],
            facing,
        });
    }
    Ok(spawns)
}
