mod client;
mod file;
mod run;
mod spec;
mod verdict;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use server::InputOrder;

pub const USAGE: &str = "\
       bots scenario FILE [--threads N] [--racy] [--wow-data DIR]
         run the scenario in FILE headless, the server's tick and every bot in this
         process on one simulated clock, as fast as it goes, and print one JSON verdict.
         Exits 0 when every expectation holds, 1 when one fails, 2 when the file is bad,
         3 when it cannot run here (its place needs the install). --threads sets the
         tick's threads (all by default); --racy applies each entity's inputs in the
         order worker threads hand them over, a control for determinism.
       A scenario file is lines of `key = value` and `expect PATH OP VALUE`, with `#` starting
         a comment: `base = FILE` lays the file over another; `place` (goldshire, elwynn or
         flat), `seconds`, `seed`, `tick_ms`, `client.delay_ms`, `client.jitter_ms`, `rules.*`
         and `view.*` set the world, and `bots.NAME.count` with the rest of `bots.NAME.*` a
         group of bots: its script, pace, spawn and lies. An expectation reads a number of the
         verdict: `NAME.FIELD` a group's, any other path the scenario's.";

/// Runs `bots scenario`'s arguments.
pub fn main(args: &[String]) -> ExitCode {
    match scenario(args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(Fault::File(e)) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Err(Fault::Here(e)) => {
            eprintln!("{e}");
            ExitCode::from(3)
        }
    }
}

enum Fault {
    File(file::Bad),
    Here(String),
}

fn scenario(args: &[String]) -> Result<bool, Fault> {
    let began = Instant::now();
    let (path, how) =
        arguments(args).map_err(|e| Fault::Here(format!("{e}\n\nusage:\n{USAGE}")))?;
    let text = file::read(&path).map_err(Fault::File)?;
    let spec = spec::spec(text, &path).map_err(Fault::File)?;
    verdict::check_paths(&spec).map_err(Fault::File)?;
    let chain = crate::install(how.data).map_err(Fault::Here)?;
    let ground = spec.place.ground(chain.as_ref()).map_err(|e| {
        Fault::Here(format!(
            "{}: {e}; set WOW_DATA to the install's Data directory",
            path.display()
        ))
    })?;
    let setup_s = began.elapsed().as_secs_f64();
    let outcome = run::run(&spec, &ground, how.threads, how.order).map_err(Fault::Here)?;
    let measured = verdict::measured(&spec, &outcome);
    let judged = verdict::judge(&spec, &measured);
    for (e, got, ok) in &judged {
        if !*ok {
            eprintln!("{}: expected {e}, got {got}", e.at);
        }
    }
    let pass = judged.iter().all(|(_, _, ok)| *ok);
    let here = verdict::Here {
        threads: how.threads,
        setup_s,
    };
    println!("{}", verdict::line(measured, &judged, &outcome, &here));
    Ok(pass)
}

struct How {
    threads: usize,
    order: InputOrder,
    data: Option<PathBuf>,
}

fn arguments(args: &[String]) -> Result<(PathBuf, How), String> {
    let (file, rest) = args.split_first().ok_or("scenario needs a FILE")?;
    let mut how = How {
        threads: std::thread::available_parallelism().map_or(1, usize::from),
        order: InputOrder::Canonical,
        data: None,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--racy" => how.order = InputOrder::Racy,
            "--threads" => {
                let n = it.next().ok_or("--threads needs a number")?;
                how.threads = n
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or(format!("--threads {n} is not a count of threads"))?;
            }
            "--wow-data" => {
                how.data = Some(it.next().ok_or("--wow-data needs a directory")?.into());
            }
            _ => return Err(format!("unexpected {arg}")),
        }
    }
    Ok((Path::new(file).to_path_buf(), how))
}
