use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::build;
use crate::command::Command;
use crate::document::{Document, Done};
use crate::grammar::{Args, point, split};
use crate::install::{Archives, Install};
use crate::query;

pub const GUIDE: &str = include_str!("guide.txt");

const SAVE_EVERY: Duration = Duration::from_secs(1);

/// Run `cairn zone` with the words after it, printing what it says; an error is the message.
pub fn main(args: &[String]) -> Result<(), String> {
    let (Globals { author, catalog }, args) = take_globals(args)?;
    let Some((verb, rest)) = args.split_first() else {
        print!("{GUIDE}");
        return Ok(());
    };
    let mut install = Archives::from_env();
    if catalog.is_some() {
        install = install.with_catalog(catalog);
    }
    let dir = |i: usize| -> Result<PathBuf, String> {
        rest.get(i)
            .map(PathBuf::from)
            .ok_or_else(|| format!("which zone? `cairn zone {verb} ZONE ...`"))
    };
    match verb.as_str() {
        "help" | "--help" | "-h" => {
            print!("{GUIDE}");
            Ok(())
        }
        "new" => new(&dir(0)?, &rest[1..], &author, &mut install),
        "replay" => {
            let (journal, dir) = (dir(0)?, dir(1)?);
            let text = std::fs::read_to_string(&journal)
                .map_err(|e| format!("{}: {e}", journal.display()))?;
            let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let t = Instant::now();
            let mut doc = Document::replay(&lines, &dir, &mut install)?;
            doc.save()?;
            println!("replayed {} lines into {}", lines.len(), dir.display());
            write_build(&doc, &mut install, t)
        }
        "undo" | "redo" => {
            let n: usize = rest
                .get(1)
                .map_or(Ok(1), |s| crate::text::number(s, verb))?;
            let step = if verb == "redo" {
                Document::redo
            } else {
                Document::undo
            };
            steps(&dir(0)?, step, n, &author, &mut install)
        }
        "run" | "batch" => {
            let dir = dir(0)?;
            let a = Args::parse(&rest[1..]);
            a.allow_only(&["journal-at-end"])?;
            let file = a.pos.first().map(PathBuf::from);
            if verb == "batch" && a.has("journal-at-end") {
                return Err("--journal-at-end is `run`'s".into());
            }
            let mut doc = Document::open(&dir, &mut install)?;
            if verb == "batch" {
                return batch(&mut doc, &author, file.as_deref(), &mut install);
            }
            run(
                &mut doc,
                &author,
                file.as_deref(),
                a.has("journal-at-end"),
                &mut install,
            )
        }
        "build" => {
            let mut doc = Document::open(&dir(0)?, &mut install)?;
            doc.save()?;
            write_build(&doc, &mut install, Instant::now())
        }
        "here" | "things" => {
            let doc = Document::open(&dir(0)?, &mut install)?;
            let mut words = vec![verb.clone()];
            words.extend(rest[1..].iter().cloned());
            print!("{}", ask(&doc, &words, &mut install)?);
            Ok(())
        }
        _ => {
            let dir = dir(0)?;
            let mut words = vec![verb.clone()];
            words.extend(rest[1..].iter().cloned());
            let cmd = Command::parse(&words, &author)?;
            let t = Instant::now();
            let mut doc = Document::open(&dir, &mut install)?;
            let done = doc.apply(&author, &cmd, false, &mut install)?;
            doc.save()?;
            println!("{}", done.reply);
            write_build(&doc, &mut install, t)
        }
    }
}

fn new(dir: &Path, rest: &[String], author: &str, install: &mut Archives) -> Result<(), String> {
    let mut words = vec!["new".to_owned()];
    words.extend(rest.iter().cloned());
    if !words.iter().any(|w| w == "--name") {
        let name = dir
            .file_name()
            .map_or("Zone".into(), |n| n.to_string_lossy().into_owned());
        words.extend(["--name".to_owned(), name]);
    }
    let Command::New(new) = Command::parse(&words, author)? else {
        return Err("not a new zone".into());
    };
    let t = Instant::now();
    let (doc, reply) = Document::create(dir, author, &new, install)?;
    println!("{reply}");
    write_build(&doc, install, t)
}

fn steps(
    dir: &Path,
    step: fn(&mut Document, &str) -> Result<Done, String>,
    n: usize,
    author: &str,
    install: &mut Archives,
) -> Result<(), String> {
    let mut doc = Document::open(dir, install)?;
    let t = Instant::now();
    for _ in 0..n {
        let one = Instant::now();
        match step(&mut doc, author) {
            Ok(done) => println!(
                "{} ({:.3} ms)",
                done.reply,
                one.elapsed().as_secs_f64() * 1e3
            ),
            Err(e) => {
                doc.save()?;
                write_build(&doc, install, t)?;
                return Err(e);
            }
        }
    }
    doc.save()?;
    write_build(&doc, install, t)
}

struct Globals {
    author: String,
    catalog: Option<PathBuf>,
}

fn take_globals(args: &[String]) -> Result<(Globals, Vec<String>), String> {
    let mut rest = Vec::with_capacity(args.len());
    let (mut named, mut catalog) = (None, None);
    let mut words = args.iter();
    while let Some(w) = words.next() {
        if w == "--as" {
            named = Some(
                words
                    .next()
                    .ok_or("--as NAME: who is changing the zone")?
                    .clone(),
            );
        } else if w == "--catalog" {
            let dir = words
                .next()
                .ok_or("--catalog DIR: the catalog `cairn catalog` wrote")?;
            catalog = Some(PathBuf::from(dir));
        } else {
            rest.push(w.clone());
        }
    }
    let name = named
        .or_else(|| std::env::var("CAIRN_AUTHOR").ok())
        .or_else(|| std::env::var("USER").ok())
        .ok_or("who is changing the zone? --as NAME, or set CAIRN_AUTHOR")?;
    crate::zone::check_author(&name)?;
    Ok((
        Globals {
            author: name,
            catalog,
        },
        rest,
    ))
}

fn write_build(doc: &Document, install: &mut dyn Install, since: Instant) -> Result<(), String> {
    let out = doc.dir().join("build");
    let b = doc.build(install)?;
    build::write(&b, &out, &doc.zone().settings.map)?;
    println!(
        "wrote {} tile{} to {} ({:.2} s)",
        b.tiles.len(),
        if b.tiles.len() == 1 { "" } else { "s" },
        out.display(),
        since.elapsed().as_secs_f64()
    );
    Ok(())
}

fn ask(doc: &Document, words: &[String], install: &mut dyn Install) -> Result<String, String> {
    let a = Args::parse(&words[1..]);
    let z = doc.zone();
    if words[0] == "here" {
        a.allow_only(&["radius"])?;
        let p = point(a.first("where? `here X,Y`")?)?;
        return query::here(z, install, p, a.num("radius")?.unwrap_or(20.0));
    }
    a.allow_only(&["at", "radius", "model"])?;
    let near = match a.one("at")? {
        Some(s) => Some((point(s)?, a.num("radius")?.unwrap_or(50.0))),
        None => None,
    };
    let model = a.words("model").map(|w| w.join(" "));
    Ok(query::things(z, install, near, model.as_deref()))
}

fn input(file: Option<&Path>) -> Result<Box<dyn BufRead>, String> {
    Ok(match file {
        Some(f) => Box::new(std::io::BufReader::new(
            std::fs::File::open(f).map_err(|e| format!("{}: {e}", f.display()))?,
        )),
        None => Box::new(std::io::stdin().lock()),
    })
}

fn run(
    doc: &mut Document,
    author: &str,
    file: Option<&Path>,
    journal_at_end: bool,
    install: &mut dyn Install,
) -> Result<(), String> {
    if journal_at_end {
        doc.hold_journal();
    }
    let started = Instant::now();
    let mut saved = Instant::now();
    let (mut done, mut failed) = (0usize, None);
    let mut out = std::io::stdout().lock();
    for (k, line) in input(file)?.lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let answer = one_line(doc, author, l, install);
        let said = match &answer {
            Ok(s) if s.contains('\n') => format!("{s}ok"),
            Ok(s) => format!("ok {s}"),
            Err(e) => format!("error: {e}"),
        };
        let _ = writeln!(out, "{said}");
        let _ = out.flush();
        match answer {
            Ok(_) => done += 1,
            Err(e) if file.is_some() => {
                failed = Some(format!("line {}: {e}", k + 1));
                break;
            }
            Err(_) => {}
        }
        if !journal_at_end && saved.elapsed() >= SAVE_EVERY {
            doc.save()?;
            saved = Instant::now();
        }
    }
    doc.release_journal()?;
    doc.save()?;
    println!("{done} lines applied");
    write_build(doc, install, started)?;
    failed.map_or(Ok(()), Err)
}

fn one_line(
    doc: &mut Document,
    author: &str,
    line: &str,
    install: &mut dyn Install,
) -> Result<String, String> {
    let words = split(line)?;
    match words.first().map(String::as_str) {
        Some("undo") if words.len() == 1 => doc.undo(author).map(|d| d.reply),
        Some("redo") if words.len() == 1 => doc.redo(author).map(|d| d.reply),
        Some("here" | "things") => ask(doc, &words, install),
        Some("build") if words.len() == 1 => {
            doc.save()?;
            let b = doc.build(install)?;
            build::write(&b, &doc.dir().join("build"), &doc.zone().settings.map)?;
            Ok(format!("wrote {} tiles", b.tiles.len()))
        }
        Some("+") => {
            let cmd = Command::parse(&words[1..], author)?;
            doc.apply(author, &cmd, true, install).map(|d| d.reply)
        }
        _ => {
            let cmd = Command::parse(&words, author)?;
            doc.apply(author, &cmd, false, install).map(|d| d.reply)
        }
    }
}

fn batch(
    doc: &mut Document,
    author: &str,
    file: Option<&Path>,
    install: &mut dyn Install,
) -> Result<(), String> {
    let started = Instant::now();
    let mut cmds = Vec::new();
    for (k, line) in input(file)?.lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let words = split(l).map_err(|e| format!("line {}: {e}", k + 1))?;
        cmds.push(Command::parse(&words, author).map_err(|e| format!("line {}: {e}", k + 1))?);
    }
    for done in doc.batch(author, &cmds, install)? {
        println!("{}", done.reply);
    }
    doc.save()?;
    write_build(doc, install, started)
}
