mod around;
mod replay;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy::app::AppExit;
use fits::{Fit, MODEL_KINDS, Spot, Tables};
use world::CurrentMap;

use crate::zone::Zone;
pub(crate) use around::{Found, Surroundings};

const DEFAULT_DIR: &str = "catalog";
const DEFAULT_MAP: &str = "Azeroth";
pub const TABLES: &str = "fits";
const DEFAULT_TOP: usize = 20;
const NEAREST_SHOWN: usize = 4;
const MARKED_LIFT: f64 = 1.5;
const FLAGS: [&str; 8] = [
    "at", "map", "zone", "replay", "kind", "borrows", "top", "sheet",
];

#[derive(Debug, PartialEq)]
pub struct Asked {
    pub dir: PathBuf,
    pub what: What,
    pub kind: Option<usize>,
    pub borrows: Option<Borrows>,
    pub top: usize,
    pub sheet: Option<PathBuf>,
}

#[derive(Debug, PartialEq)]
pub enum What {
    Install { at: [f32; 2], map: String },
    OwnZone { at: [f32; 2], root: PathBuf },
    Replay { history: PathBuf },
}

#[derive(Debug, PartialEq, Eq)]
pub enum Borrows {
    Zone(String),
    Nothing,
}

pub fn main(argv: &[String]) -> AppExit {
    let asked = match parse(argv.iter().cloned()) {
        Ok(asked) => asked,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", crate::args::USAGE);
            return AppExit::from_code(2);
        }
    };
    match run(&asked) {
        Ok(said) => {
            println!("{said}");
            AppExit::Success
        }
        Err(e) => {
            eprintln!("cairn: {e}");
            AppExit::error()
        }
    }
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Asked, String> {
    let mut args = args.into_iter();
    let mut flags: BTreeMap<String, String> = BTreeMap::new();
    let mut dir = None;
    while let Some(arg) = args.next() {
        let Some(flag) = arg.strip_prefix("--") else {
            if let Some(first) = dir.replace(PathBuf::from(&arg)) {
                return Err(format!("one catalog, not {} and {arg}", first.display()));
            }
            continue;
        };
        if !FLAGS.contains(&flag) {
            return Err(format!("unknown argument {arg}"));
        }
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        if flags.insert(flag.to_owned(), value).is_some() {
            return Err(format!("{arg} is given twice"));
        }
    }
    let mut take = |flag: &str| flags.remove(flag);
    let at = take("at")
        .map(|v| point(&v).map_err(|e| format!("--at wants {e}")))
        .transpose()?;
    let what = match (at, take("map"), take("zone"), take("replay")) {
        (Some(at), map, None, None) => What::Install {
            at,
            map: map.unwrap_or_else(|| DEFAULT_MAP.to_owned()),
        },
        (Some(at), None, Some(zone), None) => What::OwnZone {
            at,
            root: PathBuf::from(zone),
        },
        (None, None, None, Some(file)) => What::Replay {
            history: PathBuf::from(file),
        },
        (Some(_), Some(_), Some(_), _) => return Err("--map or --zone, not both".into()),
        (_, _, _, Some(_)) => return Err("--replay takes no spot, map or zone".into()),
        (None, ..) => return Err("where? --at X,Y, or --replay FILE".into()),
    };
    let kind = take("kind")
        .map(|k| {
            MODEL_KINDS
                .iter()
                .position(|known| *known == k)
                .ok_or_else(|| format!("--kind is one of {}, not {k}", MODEL_KINDS.join(", ")))
        })
        .transpose()?;
    let borrows = take("borrows").map(|b| match b.as_str() {
        "none" => Borrows::Nothing,
        _ => Borrows::Zone(b),
    });
    if matches!(what, What::Install { .. }) && borrows.is_some() {
        return Err("--borrows is for a zone of its own or a replay".into());
    }
    let top = take("top")
        .map(|n| {
            n.parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("--top wants a count above 0, not {n}"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_TOP);
    let sheet = take("sheet").map(PathBuf::from);
    if let Some(s) = &sheet
        && !s.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", s.display()));
    }
    Ok(Asked {
        dir: dir.unwrap_or_else(|| PathBuf::from(DEFAULT_DIR)),
        what,
        kind,
        borrows,
        top,
        sheet,
    })
}

fn point(value: &str) -> Result<[f32; 2], String> {
    let numbers: Option<Vec<f32>> = value
        .split(',')
        .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
        .collect();
    match numbers.as_deref() {
        Some(&[x, y]) => Ok([x, y]),
        _ => Err(format!("world X,Y in yards, not {value}")),
    }
}

fn run(asked: &Asked) -> Result<String, String> {
    let reading = Instant::now();
    let dir = asked.dir.join(TABLES);
    let tables = fits::read(&dir).map_err(|e| {
        format!(
            "{e}\nthe catalog in {} has no tables of what fits: `cairn catalog` writes them",
            asked.dir.display()
        )
    })?;
    let read_in = reading.elapsed();
    let borrows = asked.borrows.as_ref();
    let unopened = |_| "the install would not open".to_owned();
    let (mut around, at) = match &asked.what {
        What::Replay { history } => return replay::run(&tables, history, borrows),
        What::Install { at, map } => {
            let install = crate::install(None).map_err(unopened)?;
            let map = CurrentMap::find(&install.0, map)?;
            (Surroundings::on_the_install(install, &map)?, *at)
        }
        What::OwnZone { at, root } => {
            let zone = Zone::read(root)?;
            let install = crate::install(Some(root)).map_err(unopened)?;
            let around = Surroundings::of_a_zone_of_its_own(&tables, &install, &zone, borrows)?;
            (around, *at)
        }
    };
    let found = around.find(&tables, at)?;
    let ranking = Instant::now();
    let evidence = around.evidence(&tables);
    let list = evidence.list(&found.spot, asked.kind, asked.top);
    let ranked_in = ranking.elapsed();
    let mut out = found.header.clone();
    out.push_str("\nrank\tkind\tpath\twhy\tpicture\n");
    for (i, f) in list.iter().enumerate() {
        let m = &tables.models[f.model];
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            i + 1,
            m.kind,
            m.path,
            why(&tables, &found.spot, f),
            picture(&m.path)
        );
    }
    if let Some(sheet) = &asked.sheet {
        draw(asked, &tables, &found, &list, sheet)?;
        let _ = writeln!(out, "cairn: the sheet is {}", sheet.display());
    }
    let of = asked
        .kind
        .map_or("models".to_owned(), |k| format!("{}s", MODEL_KINDS[k]));
    let counted = (0..tables.models.len())
        .filter(|&m| asked.kind.is_none_or(|k| tables.kind_of(m) == Some(k)))
        .count();
    let _ = write!(
        out,
        "cairn: ranked {counted} {of} in {:.1} ms, after reading the tables in {:.0} ms",
        ranked_in.as_secs_f64() * 1000.0,
        read_in.as_secs_f64() * 1000.0
    );
    Ok(out)
}

fn header(
    tables: &Tables,
    place: &str,
    here: &survey::Underfoot,
    spot: &Spot,
    unknown: usize,
) -> String {
    let ground = match (&here.texture, here.slope) {
        (Some(t), Some(s)) => format!(", on {} sloping {}", stem(t), degrees(s)),
        (Some(t), None) => format!(", on {}", stem(t)),
        _ => String::new(),
    };
    let within = |r: f32| spot.near.iter().filter(|(_, d)| *d <= r).count();
    let nearest: Vec<String> = spot
        .near
        .iter()
        .take(NEAREST_SHOWN)
        .map(|&(m, d)| format!("{} {d:.1} yd", stem(&tables.models[m].path)))
        .collect();
    let mut said = format!(
        "cairn: {place}{ground}; {} things stand within {} yd, {} within {} yd",
        within(fits::AROUND),
        fits::AROUND,
        within(fits::NEAR),
        fits::NEAR
    );
    if !nearest.is_empty() {
        let _ = write!(said, ", the nearest {}", nearest.join(", "));
    }
    if unknown > 0 {
        let _ = write!(
            said,
            "\ncairn: {unknown} of them are models the catalog doesn't know, left out"
        );
    }
    said
}

pub(crate) fn why(tables: &Tables, spot: &Spot, f: &Fit) -> String {
    let mut parts = Vec::new();
    if f.own > 0 {
        parts.push(format!("this zone has {}", f.own));
    }
    if let Some(z) = spot.zone.filter(|_| f.in_zone > 0) {
        parts.push(format!(
            "{} places it {}",
            tables.zones[z].name,
            times(f.in_zone)
        ));
    }
    if let Some(b) = f.beside {
        let mut s = format!(
            "beside {}: {} within {} yd",
            stem(&tables.models[b.model].path),
            match b.pairs {
                1 => "1 pair".to_owned(),
                n => format!("{n} pairs"),
            },
            b.reach.yards()
        );
        if let Some(d) = b.usual {
            let _ = write!(s, ", the nearest usually {d:.1} yd from it");
        }
        parts.push(s);
    }
    if let (Some(lift), Some((g, band))) = (f.ground, spot.ground) {
        let on = format!(
            "{} at {} degrees",
            stem(&tables.grounds[g]),
            fits::band_name(band)
        );
        if lift >= MARKED_LIFT {
            parts.push(format!("{lift:.1}x as often on {on} as anywhere"));
        } else if lift <= 1.0 / MARKED_LIFT {
            parts.push(format!("seldom on {on}"));
        }
    }
    if parts.is_empty() {
        parts.push("nothing speaks for it".to_owned());
    }
    parts.join("; ")
}

fn degrees(d: f32) -> String {
    match d.round() {
        1.0 => "1 degree".to_owned(),
        d => format!("{d:.0} degrees"),
    }
}

fn times(n: u32) -> String {
    match n {
        1 => "once".to_owned(),
        n => format!("{n} times"),
    }
}

pub(crate) fn stem(path: &str) -> &str {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    name.rsplit_once('.').map_or(name, |(s, _)| s)
}

fn picture(path: &str) -> String {
    format!("models/{}.png", survey::key(path))
}

fn draw(
    asked: &Asked,
    tables: &Tables,
    found: &Found,
    list: &[Fit],
    out: &Path,
) -> Result<(), String> {
    let cells: Vec<survey::Cell> = list
        .iter()
        .map(|f| {
            let path = &tables.models[f.model].path;
            let mut facts = if f.own > 0 {
                format!("{} here", f.own)
            } else {
                format!("{} in zone", f.in_zone)
            };
            if let Some(b) = f.beside {
                let _ = write!(facts, ", ");
                if let Some(d) = b.usual {
                    let _ = write!(facts, "{d:.0} yd ");
                }
                let _ = write!(facts, "by {}", stem(&tables.models[b.model].path));
            }
            survey::Cell {
                picture: asked.dir.join(picture(path)),
                name: stem(path).to_owned(),
                facts,
            }
        })
        .collect();
    let at = match &asked.what {
        What::Install { at, .. } | What::OwnZone { at, .. } => *at,
        What::Replay { .. } => [0.0; 2],
    };
    let of = asked
        .kind
        .map_or("every kind".to_owned(), |k| format!("{}s", MODEL_KINDS[k]));
    let title = format!(
        "what fits at {:.0}, {:.0} ({}): {of}, 1 to {}",
        at[0],
        at[1],
        found.sheet_place,
        cells.len()
    );
    survey::draw_page(&title, 1, &cells, out)
}

#[cfg(test)]
mod tests;
