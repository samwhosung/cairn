use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::str::FromStr;

use crate::{Model, Pair, SLOPE_NAMES, Tables, Zone};

/// The files the tables are written to, each with the header it opens with.
pub const FILES: [(&str, &str); 6] = [
    ("models.tsv", "model\tkind\tpath"),
    ("grounds.tsv", "ground\tpath"),
    ("zones.tsv", "zone\tmap\tarea\tkey\tname"),
    ("palette.tsv", "zone\tmodel\tplaced"),
    ("ground.tsv", "ground\tslope\tmodel\tplaced"),
    (
        "near.tsv",
        "model\tbeside\twithin 8 yd\twithin 20 yd\tyd to the nearest\tyd back",
    ),
];

/// Writes the files `dir` lacks, each whole or not at all, from the tables `count` makes, which it
/// is asked for only when one is missing. Says how many it wrote.
pub fn write(dir: &Path, count: impl FnOnce() -> Tables) -> Result<usize, String> {
    let missing: Vec<&str> = FILES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !dir.join(name).exists())
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    let tables = count();
    for name in &missing {
        let body = text(&tables, name);
        survey::write_atomically(&dir.join(name), |part| {
            std::fs::write(part, body).map_err(|e| format!("{}: {e}", part.display()))
        })?;
    }
    Ok(missing.len())
}

fn text(t: &Tables, name: &str) -> String {
    let header = FILES
        .iter()
        .find(|(n, _)| *n == name)
        .map_or("", |(_, h)| *h);
    let mut out = format!("{header}\n");
    match name {
        "models.tsv" => {
            for (i, m) in t.models.iter().enumerate() {
                let _ = writeln!(out, "{i}\t{}\t{}", m.kind, m.path);
            }
        }
        "grounds.tsv" => {
            for (i, g) in t.grounds.iter().enumerate() {
                let _ = writeln!(out, "{i}\t{g}");
            }
        }
        "zones.tsv" => {
            for (i, z) in t.zones.iter().enumerate() {
                let _ = writeln!(out, "{i}\t{}\t{}\t{}\t{}", z.map, z.area, z.key, z.name);
            }
        }
        "palette.tsv" => {
            for (z, list) in t.palette.iter().enumerate() {
                for (m, n) in list {
                    let _ = writeln!(out, "{z}\t{m}\t{n}");
                }
            }
        }
        "ground.tsv" => {
            for (&(g, band), list) in &t.ground {
                for (m, n) in list {
                    let _ = writeln!(out, "{g}\t{}\t{m}\t{n}", crate::band_name(band));
                }
            }
        }
        _ => {
            for p in &t.pairs {
                let _ = writeln!(
                    out,
                    "{}\t{}\t{}\t{}\t{:.1}\t{:.1}",
                    p.a, p.b, p.near, p.around, p.a_to_b, p.b_to_a
                );
            }
        }
    }
    out
}

/// The tables `write` wrote into `dir`.
pub fn read(dir: &Path) -> Result<Tables, String> {
    let file = |name: &str| {
        let path = dir.join(name);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok::<_, String>((path.display().to_string(), text))
    };
    let (at, text) = file("models.tsv")?;
    let models = rows(&at, &text, 3, |i, f| {
        numbered(i, f[0])?;
        Ok(Model {
            kind: f[1].to_owned(),
            path: f[2].to_owned(),
        })
    })?;
    let (at, text) = file("grounds.tsv")?;
    let grounds = rows(&at, &text, 2, |i, f| {
        numbered(i, f[0])?;
        Ok(f[1].to_owned())
    })?;
    let (at, text) = file("zones.tsv")?;
    let zones = rows(&at, &text, 5, |i, f| {
        numbered(i, f[0])?;
        Ok(Zone {
            map: number(f[1])?,
            area: number(f[2])?,
            key: f[3].to_owned(),
            name: f[4].to_owned(),
        })
    })?;
    let model = |s: &str| index(s, models.len());
    let (at, text) = file("palette.tsv")?;
    let mut palette = vec![Vec::new(); zones.len()];
    for (z, m, n) in rows(&at, &text, 3, |_, f| {
        Ok((index(f[0], zones.len())?, model(f[1])?, number(f[2])?))
    })? {
        palette[z].push((m, n));
    }
    let (at, text) = file("ground.tsv")?;
    let mut ground: BTreeMap<(usize, u8), Vec<(usize, u32)>> = BTreeMap::new();
    for (g, band, m, n) in rows(&at, &text, 4, |_, f| {
        let band = SLOPE_NAMES
            .iter()
            .position(|b| *b == f[1])
            .ok_or_else(|| format!("no slope band {}", f[1]))?;
        Ok((
            index(f[0], grounds.len())?,
            band as u8,
            model(f[2])?,
            number(f[3])?,
        ))
    })? {
        ground.entry((g, band)).or_default().push((m, n));
    }
    let (at, text) = file("near.tsv")?;
    let pairs = rows(&at, &text, 6, |_, f| {
        let (a, b) = (model(f[0])?, model(f[1])?);
        if a > b {
            return Err(format!("{a} after {b}"));
        }
        Ok(Pair {
            a,
            b,
            near: number(f[2])?,
            around: number(f[3])?,
            a_to_b: number(f[4])?,
            b_to_a: number(f[5])?,
        })
    })?;
    ordered(&at, &pairs, |p| (p.a, p.b))?;
    for (z, list) in palette.iter().enumerate() {
        ordered(
            &format!("{dir}/palette.tsv, zone {z}", dir = dir.display()),
            list,
            |e| e.0,
        )?;
    }
    for list in ground.values() {
        ordered(&format!("{}/ground.tsv", dir.display()), list, |e| e.0)?;
    }
    Ok(Tables::from_parts(
        models, grounds, zones, palette, ground, pairs,
    ))
}

fn rows<T>(
    at: &str,
    text: &str,
    fields: usize,
    parse: impl Fn(usize, &[&str]) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or_default();
    if header.split('\t').count() != fields {
        return Err(format!("{at}: the header is not {fields} columns"));
    }
    lines
        .enumerate()
        .map(|(i, line)| {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != fields {
                return Err(format!("{at}:{}: want {fields} columns", i + 2));
            }
            parse(i, &f).map_err(|e| format!("{at}:{}: {e}", i + 2))
        })
        .collect()
}

fn numbered(row: usize, s: &str) -> Result<(), String> {
    if number::<usize>(s)? == row {
        Ok(())
    } else {
        Err(format!("row {row} is numbered {s}"))
    }
}

fn number<T: FromStr>(s: &str) -> Result<T, String> {
    s.parse().map_err(|_| format!("`{s}` is not a number"))
}

fn index(s: &str, len: usize) -> Result<usize, String> {
    let i: usize = number(s)?;
    if i < len {
        Ok(i)
    } else {
        Err(format!("{i} is past the {len} listed"))
    }
}

/// Lookups search the lists, so each must be in order, once each.
fn ordered<T, K: Ord>(at: &str, list: &[T], key: impl Fn(&T) -> K) -> Result<(), String> {
    match list.windows(2).position(|w| key(&w[0]) >= key(&w[1])) {
        Some(i) => Err(format!("{at}: row {} is out of order", i + 2)),
        None => Ok(()),
    }
}
