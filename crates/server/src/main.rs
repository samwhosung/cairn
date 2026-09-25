//! Serves one world over TCP, or replays a recorded run.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use server::{
    Config, InputOrder, Limits, Refusal, Replay, Replicate, Spawn, Summary, Window, ground_between,
};

const USAGE: &str = "\
usage: server [--port P] [--threads N] [--io-threads N] [--spawns FILE] [--unchecked]
              [--record FILE] [--players N --arrival S --settle S --measure S --grace S]
              [--game NAME [--knobs FILE] [--overlay FILE] [--seed N]] [--label TEXT]
         serve a world on 127.0.0.1:P (7777 by default). With --players, once N players
         are in, or S seconds after the start (--arrival, 60) with whoever is, wait S
         seconds and measure for S seconds; once every player has left, or S seconds
         after the window (--grace, 10) with the rest dropped, print one summary row and
         stop.
         --unchecked accepts every well-formed claim. --record writes every tick's inputs
         and world hash to FILE. --game runs a game's rules (melee) on its own knobs, or on
         the --knobs FILE, with an --overlay FILE laid on them; --seed seeds its rolls.
       server header
         print the header of the summary row's table.
       server replay FILE [--threads N] [--racy] [--refusals] [--replicate] [--dump OUT]
         replay a recorded run and compare the world after every tick; --racy applies
         each entity's inputs in the order worker threads hand them over; --refusals
         prints every refused claim and what it was judged against; --replicate also
         builds every client's batch as if all kept up and prints the summary row;
         --dump writes the first connection's frames to OUT, and implies --replicate.

FILE of spawns: one `x y z facing` per line, WoW world coordinates and radians.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("replay") => replay(&args[1..]),
        Some("header") => {
            println!("{}", Summary::header());
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
            arrival: ticks(num(&f, "arrival", 60.0)?),
            settle: ticks(num(&f, "settle", 5.0)?),
            measure: ticks(num(&f, "measure", 20.0)?),
            grace: ticks(num(&f, "grace", 10.0)?),
        }),
        None => None,
    };
    let spawns = match f.get("spawns") {
        Some(path) => read_spawns(Path::new(path))?,
        None => Vec::new(),
    };
    let game = match f.get("game") {
        Some(name) => {
            let (knobs, overlay) = (f.get("knobs"), f.get("overlay"));
            let seed = num(&f, "seed", 0)?;
            let files = (knobs.map(Path::new), overlay.map(Path::new));
            Some(catalog::from_files(name, files.0, files.1, seed)?)
        }
        None if ["knobs", "overlay", "seed"]
            .iter()
            .any(|k| f.contains_key(*k)) =>
        {
            return Err("--knobs, --overlay and --seed are a game's: give --game".into());
        }
        None => None,
    };
    let cfg = Config {
        addr: Some(SocketAddr::from(([127, 0, 0, 1], num(&f, "port", 7777)?))),
        tick_threads: num(&f, "threads", defaults.tick_threads)?,
        io_threads: num(&f, "io-threads", defaults.io_threads)?,
        spawns,
        limits: Limits {
            check: !f.contains_key("unchecked"),
            ..Limits::default()
        },
        record: f.get("record").map(PathBuf::from),
        window,
        game,
        ..defaults
    };
    let running = server::start(cfg).map_err(|e| format!("starting: {e}"))?;
    if let Some(addr) = running.addr() {
        eprintln!("serving on {addr}");
    }
    let summary = running.wait().map_err(|e| format!("serving: {e}"))?;
    if summary.players_arrived < summary.players_wanted {
        eprintln!(
            "{} of the {} players were in when the window stopped waiting for them",
            summary.players_arrived, summary.players_wanted
        );
    }
    if window.is_some_and(|w| summary.ticks == w.measure as usize) {
        let after = f64::from(summary.ticks_after) * f64::from(tick_ms) / 1000.0;
        match summary.stayed {
            0 => eprintln!("every player had left {after:.2} s after the window"),
            n => eprintln!("the grace ran out {after:.2} s after the window: dropped {n} still in"),
        }
    }
    let label = f.get("label").map_or("", String::as_str);
    println!("{}", summary.row(label));
    Ok(())
}

fn replay(args: &[String]) -> Result<(), String> {
    let (path, rest) = args.split_first().ok_or("replay needs a FILE")?;
    let f = flags(rest, &["racy", "refusals", "replicate"])?;
    let dump = f.get("dump").map(PathBuf::from);
    let how = Replay {
        threads: num(&f, "threads", 1)?,
        order: if f.contains_key("racy") {
            InputOrder::Racy
        } else {
            InputOrder::Canonical
        },
        keep_refusals: f.contains_key("refusals"),
        replicate: match &dump {
            Some(path) => Replicate::Dumping(path),
            None if f.contains_key("replicate") => Replicate::Yes,
            None => Replicate::No,
        },
    };
    let r = server::replay(Path::new(path), &how).map_err(|e| format!("{path}: {e}"))?;
    let limits = Limits::default();
    for refusal in &r.refusals {
        println!("{}", refusal_line(refusal, &limits));
    }
    let verdict = r
        .first_mismatch
        .map_or("every tick matches".to_string(), |t| {
            format!("differs from tick {t}")
        });
    println!(
        "replay threads={} order={:?} ticks={} hash={:016x} {verdict}",
        how.threads, how.order, r.ticks, r.hash
    );
    if how.replicate != Replicate::No {
        println!("{}", r.summary.row(&format!("replay {path}")));
    }
    Ok(())
}

fn refusal_line(r: &Refusal, limits: &Limits) -> String {
    let at = |m: &server::Movement| {
        format!(
            "t={} flags={:#x} ({:.2}, {:.2}, {:.2})",
            m.time, m.flags, m.pos[0], m.pos[1], m.pos[2]
        )
    };
    format!(
        "tick {} id {} {} {:?} (received at {} ms): {} -> {}: {:.2} yd over the ground in {} ms, {:.2} allowed",
        r.tick,
        r.id,
        r.name,
        r.why,
        r.received_ms,
        at(&r.last),
        at(&r.claim),
        ground_between(&r.last, &r.claim),
        r.claim.time.saturating_sub(r.last.time),
        limits.ground_allowed(&r.last, &r.claim),
    )
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
