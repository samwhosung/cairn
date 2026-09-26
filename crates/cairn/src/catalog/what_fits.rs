mod replay;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy::app::AppExit;
use fits::{Evidence, Fit, KINDS, Own, Spot, Tables};
use terrain::TileMesh;
use world::{CurrentMap, Install};

use crate::zone::Zone;

const DEFAULT_DIR: &str = "catalog";
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
    /// A spot on a map of the install's, Azeroth unless named.
    Install { at: [f32; 2], map: Option<String> },
    /// A spot in a zone of its own.
    Own { at: [f32; 2], zone: PathBuf },
    /// A zone's placements in the order they were made.
    Replay(PathBuf),
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
        (Some(at), map, None, None) => What::Install { at, map },
        (Some(at), None, Some(zone), None) => What::Own {
            at,
            zone: PathBuf::from(zone),
        },
        (None, None, None, Some(file)) => What::Replay(PathBuf::from(file)),
        (Some(_), Some(_), Some(_), _) => return Err("--map or --zone, not both".into()),
        (_, _, _, Some(_)) => return Err("--replay takes no spot, map or zone".into()),
        (None, ..) => return Err("where? --at X,Y, or --replay FILE".into()),
    };
    let kind = take("kind")
        .map(|k| {
            KINDS
                .iter()
                .position(|known| *known == k)
                .ok_or_else(|| format!("--kind is one of {}, not {k}", KINDS.join(", ")))
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
    let found = match &asked.what {
        What::Replay(file) => return replay::run(&tables, file, borrows),
        What::Install { at, map } => on_the_install(&tables, *at, map.as_deref())?,
        What::Own { at, zone } => in_a_zone_of_its_own(&tables, *at, zone, borrows)?,
    };
    let ranking = Instant::now();
    let evidence = Evidence::new(&tables, &found.own, found.install);
    let list = evidence.list(&found.spot, asked.kind, asked.top);
    let ranked_in = ranking.elapsed();
    let mut out = found.said.clone();
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
        .map_or("models".to_owned(), |k| format!("{}s", KINDS[k]));
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

/// A spot to rank for, what counts toward its list, and what to say of it.
struct Found {
    spot: Spot,
    own: Own,
    install: bool,
    said: String,
    /// The place in a few words, for a sheet's title.
    place: String,
}

fn on_the_install(tables: &Tables, at: [f32; 2], map: Option<&str>) -> Result<Found, String> {
    let install = crate::install(None).map_err(|_| "the install would not open".to_owned())?;
    let chain = &install.0;
    let map = CurrentMap::find(chain, map.unwrap_or("Azeroth"))?;
    let tiles = tiles_around(&install, &map.directory, at);
    let here = underfoot(&tiles, at)
        .ok_or_else(|| format!("{},{} has no ground on {}", at[0], at[1], map.directory))?;
    let areas = atlas::Areas::load(chain)?;
    let zone = tables.zone(map.id, areas.top_zone(here.area).unwrap_or(0));
    let mut near = Vec::new();
    let mut unknown = 0;
    for (m, xy) in standing(&tiles) {
        let d = (xy[0] - at[0]).hypot(xy[1] - at[1]);
        if d > fits::AROUND {
            continue;
        }
        match tables.model(&m) {
            Some(m) => near.push((m, d)),
            None => unknown += 1,
        }
    }
    near.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let spot = Spot {
        zone,
        ground: ground_here(tables, &here),
        near,
    };
    let place = zone
        .map_or("no zone", |z| tables.zones[z].name.as_str())
        .to_owned();
    let where_ = format!("{},{} on {}, in {place}", at[0], at[1], map.directory);
    Ok(Found {
        said: header(tables, &where_, &here, &spot, unknown),
        spot,
        own: Own::default(),
        install: true,
        place,
    })
}

fn in_a_zone_of_its_own(
    tables: &Tables,
    at: [f32; 2],
    root: &Path,
    borrows: Option<&Borrows>,
) -> Result<Found, String> {
    let zone = Zone::read(root)?;
    let install =
        crate::install(Some(root)).map_err(|_| "the install would not open".to_owned())?;
    let tiles = atlas::load_map(&install.0, &zone.directory);
    if tiles.is_empty() {
        return Err(format!("no tile of {} reads", zone.directory));
    }
    let (own, unknown) = placed_in(tables, &tiles);
    let borrowed = match borrows {
        Some(Borrows::Nothing) => None,
        Some(Borrows::Zone(name)) => Some(name.as_str()),
        None => Some(zone.borrows.as_str()),
    };
    let palette = borrowed
        .map(|name| {
            tables
                .zone_named(name)
                .ok_or_else(|| format!("the catalog has no zone named {name}"))
        })
        .transpose()?;
    let here = underfoot(&tiles, at)
        .ok_or_else(|| format!("{},{} is off {}'s ground", at[0], at[1], zone.directory))?;
    let spot = Spot {
        zone: palette,
        ground: ground_here(tables, &here),
        near: own.around(at),
    };
    let place = palette.map_or_else(
        || format!("{}, by its own placements alone", zone.directory),
        |z| format!("{}, borrowing {}", zone.directory, tables.zones[z].name),
    );
    let where_ = format!(
        "{},{} in {place}, a zone of its own with {} things",
        at[0],
        at[1],
        own.len()
    );
    Ok(Found {
        said: header(tables, &where_, &here, &spot, unknown),
        spot,
        own,
        install: palette.is_some(),
        place,
    })
}

fn underfoot(tiles: &[((u32, u32), TileMesh)], at: [f32; 2]) -> Option<survey::Underfoot> {
    let tile = wdt::world_to_tile(at[0], at[1]);
    let (_, mesh) = tiles.iter().find(|(t, _)| *t == tile)?;
    survey::underfoot(&mesh.chunks, tile, [at[0], at[1], 0.0])
}

/// What a zone of its own has placed, each thing once from the tile it stands on, and how many
/// things are of models the catalog doesn't know.
fn placed_in(tables: &Tables, tiles: &[((u32, u32), TileMesh)]) -> (Own, usize) {
    let mut things: BTreeMap<String, Thing> = BTreeMap::new();
    let mut unknown = BTreeSet::new();
    for (tile, mesh) in tiles {
        for (id, model, p) in placed_on(mesh) {
            let Some(model) = tables.model(model) else {
                unknown.insert(id);
                continue;
            };
            let under = survey::underfoot(&mesh.chunks, *tile, p);
            let ground = under.as_ref().and_then(|u| ground_here(tables, u));
            let at = [p[0], p[1]];
            let thing = things.entry(id).or_insert(Thing { model, at, ground });
            if under.is_some() {
                *thing = Thing { model, at, ground };
            }
        }
    }
    let mut own = Own::default();
    for (id, t) in &things {
        own.place(id, t.model, t.at, t.ground);
    }
    (own, unknown.len())
}

struct Thing {
    model: usize,
    at: [f32; 2],
    ground: Option<(usize, u8)>,
}

fn ground_here(tables: &Tables, here: &survey::Underfoot) -> Option<(usize, u8)> {
    let texture = tables.ground_texture(here.texture.as_deref()?)?;
    Some((texture, fits::band(here.slope?)))
}

/// Every tile of `directory` within reach of `at`.
fn tiles_around(install: &Install, directory: &str, at: [f32; 2]) -> Vec<((u32, u32), TileMesh)> {
    let reach = fits::AROUND;
    let mut wanted: Vec<(u32, u32)> = [-reach, reach]
        .iter()
        .flat_map(|dx| [-reach, reach].map(|dy| wdt::world_to_tile(at[0] + dx, at[1] + dy)))
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    wanted
        .into_iter()
        .filter_map(|(x, y)| {
            let mesh = terrain::load_tile_mesh(&install.0, directory, x, y).ok()?;
            Some(((x, y), mesh))
        })
        .collect()
}

/// Each doodad and building on the tiles once, by id, with where it stands.
fn standing(tiles: &[((u32, u32), TileMesh)]) -> Vec<(String, [f32; 2])> {
    let mut by_id: BTreeMap<String, (String, [f32; 2])> = BTreeMap::new();
    for (_, mesh) in tiles {
        for (id, model, p) in placed_on(mesh) {
            by_id.insert(id, (model.to_owned(), [p[0], p[1]]));
        }
    }
    by_id.into_values().collect()
}

fn placed_on(mesh: &TileMesh) -> impl Iterator<Item = (String, &str, [f32; 3])> {
    let doodads = mesh.doodads.iter().map(|d| {
        (
            format!("doodad {}", d.unique_id),
            d.model.as_str(),
            d.position,
        )
    });
    let buildings = mesh.wmos.iter().map(|w| {
        (
            format!("building {}", w.unique_id),
            w.model.as_str(),
            w.position,
        )
    });
    doodads.chain(buildings)
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

fn why(tables: &Tables, spot: &Spot, f: &Fit) -> String {
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
            b.within
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

fn stem(path: &str) -> &str {
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
        What::Install { at, .. } | What::Own { at, .. } => *at,
        What::Replay(_) => [0.0; 2],
    };
    let of = asked
        .kind
        .map_or("every kind".to_owned(), |k| format!("{}s", KINDS[k]));
    let title = format!(
        "what fits at {:.0}, {:.0} ({}): {of}, 1 to {}",
        at[0],
        at[1],
        found.place,
        cells.len()
    );
    survey::draw_page(&title, 1, &cells, out)
}

#[cfg(test)]
mod tests;
