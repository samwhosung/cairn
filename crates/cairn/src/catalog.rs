//! `cairn catalog`: the install's ground textures, doodads, buildings and zones written out for an
//! agent to search and look through.

mod studio;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy::app::AppExit;
use sound::tables::{AreaSounds, Kit, KitCatalog};
use survey::{Model, Zone, ZoneSound};
use world::Install;

use studio::Sitter;

const DEFAULT_DIR: &str = "catalog";
/// A model's picture is this many pixels square.
const PICTURE: u32 = 480;
/// The first line of `catalog.txt`: a catalog written by another version is not added to.
const FORMAT: &str = "cairn catalog 1";

#[derive(Debug, PartialEq)]
pub struct Order {
    pub dir: PathBuf,
    /// Draw at most this many of the missing pictures, then stop.
    pub draw: Option<usize>,
}

pub fn main(argv: &[String]) -> AppExit {
    let order = match parse(argv.iter().cloned()) {
        Ok(order) => order,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", crate::args::USAGE);
            return AppExit::from_code(2);
        }
    };
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("cairn: set WOW_DATA to the Data directory of a WoW 1.12.1 install");
        return AppExit::from_code(2);
    };
    let install = match crate::install(None) {
        Ok(install) => install,
        Err(exit) => return exit,
    };
    match run(&order, &install, &data) {
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

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Order, String> {
    let mut args = args.into_iter();
    let (mut dir, mut draw) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--draw" if draw.is_none() => {
                let n = args.next().ok_or("--draw needs a count")?;
                draw = Some(
                    n.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("--draw wants a count of pictures, not {n}"))?,
                );
            }
            "--draw" => return Err("--draw is given twice".into()),
            flag if flag.starts_with("--") => return Err(format!("unknown argument {flag}")),
            _ => {
                if let Some(first) = dir.replace(PathBuf::from(&arg)) {
                    return Err(format!("one directory, not {} and {arg}", first.display()));
                }
            }
        }
    }
    Ok(Order {
        dir: dir.unwrap_or_else(|| PathBuf::from(DEFAULT_DIR)),
        draw,
    })
}

fn run(order: &Order, install: &Install, data: &Path) -> Result<String, String> {
    let started = Instant::now();
    let chain = &install.0;
    let inv = survey::read(chain)?;
    let read_in = started.elapsed();
    stamp(&order.dir, data)?;
    let sounds = Sounds::load(install)?;
    let px_per_yard = |m: &Model| m.bounds.map(|b| PICTURE as f32 / studio::frame(b).side);
    let text = survey::write(&inv, chain, &order.dir, &|z| sounds.of(z), &px_per_yard)?;
    let missing = survey::pictures_missing(&inv, &order.dir);
    let to_draw: Vec<Sitter> = missing
        .iter()
        .take(order.draw.unwrap_or(usize::MAX))
        .filter_map(|&i| sitter(&inv.models[i], &order.dir))
        .collect();
    let drawing = Instant::now();
    let drawn = if to_draw.is_empty() {
        Vec::new()
    } else {
        studio::draw(install, to_draw, PICTURE)?
    };
    let drawn_in = drawing.elapsed();
    for d in drawn.iter().filter(|d| d.failed.is_some()) {
        eprintln!(
            "cairn: {}: {}",
            d.path,
            d.failed.as_deref().unwrap_or_default()
        );
    }
    let still = survey::pictures_missing(&inv, &order.dir).len();
    let pages = if still == 0 && order.draw.is_none() {
        Some(survey::write_pages(&inv, &order.dir)?)
    } else {
        None
    };
    let buildings = inv.models.iter().filter(|m| m.building).count();
    let mut said = format!(
        "cairn: {} models ({buildings} buildings), {} ground textures and {} zones read in {:.1} s\n\
         cairn: text, swatches and skies: wrote {}, kept {}\n\
         cairn: drew {} pictures in {:.1} s; {still} still to draw",
        inv.models.len(),
        inv.grounds.len(),
        inv.zones.len(),
        read_in.as_secs_f32(),
        text.wrote,
        text.kept,
        drawn.len(),
        drawn_in.as_secs_f32(),
    );
    match pages {
        Some(p) => {
            let _ = write!(said, "\ncairn: pages: wrote {}, kept {}", p.wrote, p.kept);
        }
        None if still > 0 => said.push_str("\ncairn: the pages wait for every picture: run again"),
        None => said.push_str("\ncairn: the pages wait for a run without --draw"),
    }
    let _ = write!(
        said,
        "\ncairn: the catalog is in {} ({:.1} s)",
        order.dir.display(),
        started.elapsed().as_secs_f32()
    );
    Ok(said)
}

fn sitter(m: &Model, dir: &Path) -> Option<Sitter> {
    Some(Sitter {
        path: m.path.clone(),
        building: m.building,
        bounds: m.bounds?,
        out: dir.join(format!("models/{}.png", m.key)),
    })
}

/// Writes `catalog.txt`, naming the format and the install's archives, or checks the one there
/// names the same: a catalog of another install or format is never added to.
fn stamp(dir: &Path, data: &Path) -> Result<(), String> {
    let mut archives: Vec<(String, u64)> = std::fs::read_dir(data)
        .map_err(|e| format!("{}: {e}", data.display()))?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let is_archive = Path::new(&name)
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("mpq"));
            is_archive.then(|| Some((name, e.metadata().ok()?.len())))?
        })
        .collect();
    archives.sort();
    let mut want = format!("{FORMAT}\nthe install's archives:\n");
    for (name, size) in &archives {
        let _ = writeln!(want, "  {name} {size}");
    }
    let path = dir.join("catalog.txt");
    match std::fs::read_to_string(&path) {
        Ok(have) if have == want => Ok(()),
        Ok(_) => Err(format!(
            "{} holds a catalog of another install or format; delete it or name another directory",
            dir.display()
        )),
        Err(_) => survey::write_atomically(&path, |part| {
            std::fs::write(part, &want).map_err(|e| format!("{}: {e}", part.display()))
        }),
    }
}

/// The sound tables, which name a zone's music and ambience.
struct Sounds {
    areas: AreaSounds,
    kits: KitCatalog,
}

impl Sounds {
    fn load(install: &Install) -> Result<Self, String> {
        Ok(Self {
            areas: AreaSounds::load(&install.0).map_err(|e| e.to_string())?,
            kits: KitCatalog::load(&install.0).map_err(|e| e.to_string())?,
        })
    }

    fn kit(&self, id: u32) -> Option<&Kit> {
        (id != 0).then(|| self.kits.get(id)).flatten()
    }

    fn lines(&self, [day, night]: [u32; 2]) -> Vec<String> {
        [("by day", day), ("by night", night)]
            .into_iter()
            .filter_map(|(when, id)| {
                let kit = self.kit(id)?;
                let files: Vec<&str> = kit.files.iter().map(|(f, _)| f.as_str()).collect();
                Some(format!("{when}: {}: {}", kit.name, files.join(", ")))
            })
            .collect()
    }

    fn of(&self, z: &Zone) -> ZoneSound {
        let Some(audio) = self.areas.resolve(z.area) else {
            return ZoneSound::default();
        };
        let (music, music_lines) = audio.music.map_or_else(Default::default, |m| {
            (m.set_name.clone(), self.lines(m.sounds))
        });
        let (ambience, ambience_lines) = audio.ambience.map_or_else(Default::default, |a| {
            let name = self.kit(a[0]).map(|k| k.name.clone()).unwrap_or_default();
            (name, self.lines(a))
        });
        ZoneSound {
            music,
            music_lines,
            ambience,
            ambience_lines,
        }
    }
}

#[cfg(test)]
mod tests;
