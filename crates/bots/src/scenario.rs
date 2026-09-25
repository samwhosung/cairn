mod client;
mod file;
mod played;
mod run;
mod shown;
mod spec;
mod verdict;

use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use game::Delivery;
use server::{InputOrder, Summary};

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
    let (path, how) = arguments(args).map_err(|what| {
        Fault::File(file::Bad {
            at: None,
            what: format!("{what}\n\n{}", crate::USAGE),
        })
    })?;
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
    let outcome = run::run(&spec, &ground, how.threads, how.orders).map_err(Fault::Here)?;
    let measured = verdict::measured(&spec, &outcome);
    let judged = verdict::judge(&spec, &measured);
    for j in judged.iter().filter(|j| !j.held) {
        eprintln!("{}: expected {}, got {}", j.expect.at, j.expect, j.got);
    }
    let pass = judged.iter().all(|j| j.held);
    let here = verdict::ThisMachine {
        threads: how.threads,
        setup_s,
    };
    println!("{}", verdict::line(measured, &judged, &outcome, &here));
    if let Some(file) = &how.hashes {
        let mut lines = String::new();
        for t in &outcome.ticks {
            let _ = writeln!(lines, "{} {:016x}", t.tick, t.hash);
        }
        std::fs::write(file, lines).map_err(|e| Fault::Here(format!("{}: {e}", file.display())))?;
    }
    if how.row {
        let game_s = outcome.ticks.len() as f64 * f64::from(outcome.tick_ms) / 1000.0;
        let summary = Summary::of(&outcome.ticks, how.threads, game_s, 0, 0);
        eprintln!("{}", summary.row(&spec.name));
    }
    Ok(pass)
}

struct How {
    threads: usize,
    orders: run::Orders,
    row: bool,
    hashes: Option<PathBuf>,
    data: Option<PathBuf>,
}

fn arguments(args: &[String]) -> Result<(PathBuf, How), String> {
    let (file, rest) = args.split_first().ok_or("scenario needs a FILE")?;
    let mut how = How {
        threads: std::thread::available_parallelism().map_or(1, usize::from),
        orders: run::Orders {
            inputs: InputOrder::Canonical,
            letters: Delivery::Canonical,
        },
        row: false,
        hashes: None,
        data: None,
    };
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--racy" => how.orders.inputs = InputOrder::Racy,
            "--reversed" => how.orders.letters = Delivery::Reversed,
            "--row" => how.row = true,
            "--hashes" => {
                how.hashes = Some(it.next().ok_or("--hashes needs a file")?.into());
            }
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
