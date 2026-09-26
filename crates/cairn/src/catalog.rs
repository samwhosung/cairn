mod studio;
pub(crate) mod what_fits;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use atlas::Areas;
use bevy::app::AppExit;
use sound::tables::{AreaSounds, Kit, KitCatalog};
use survey::{Lookups, Model, Zone, ZoneSound};
use world::Install;

use studio::Sitter;

pub(crate) const DEFAULT_DIR: &str = "catalog";
const CATALOG_VERSION: &str = "cairn catalog 1";

#[derive(Debug, PartialEq)]
pub struct Order {
    pub dir: PathBuf,
    pub draw_at_most: Option<usize>,
}

pub fn main(argv: &[String]) -> AppExit {
    if argv.first().is_some_and(|arg| arg == "fits") {
        return what_fits::main(&argv[1..]);
    }
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
    let (mut dir, mut draw_at_most) = (None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--draw" if draw_at_most.is_none() => {
                let n = args.next().ok_or("--draw needs a count")?;
                draw_at_most = Some(
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
        draw_at_most,
    })
}

fn run(order: &Order, install: &Install, data: &Path) -> Result<String, String> {
    let started = Instant::now();
    let chain = &install.0;
    let inv = survey::read(chain)?;
    let read_in = started.elapsed();
    stamp_or_check(&order.dir, data)?;
    let lookups = InstallLookups {
        install,
        areas: Areas::load(chain)?,
        sounds: SoundTables::load(install)?,
    };
    let text = survey::write(&inv, chain, &order.dir, &lookups)?;
    let counting = Instant::now();
    let in_catalog = order.dir.join(fits::IN_CATALOG);
    let tables = fits::write(&in_catalog, || fits::Tables::of(&inv))?;
    let rules = fits::rules::write(&in_catalog, || fits::rules::of(&inv))?;
    let counted_in = counting.elapsed();
    let missing = survey::pictures_missing(&inv, &order.dir);
    let to_draw: Vec<Sitter> = missing
        .iter()
        .take(order.draw_at_most.unwrap_or(usize::MAX))
        .filter_map(|&i| sitter(&inv.models[i], &order.dir))
        .collect();
    let drawing = Instant::now();
    let drawn = if to_draw.is_empty() {
        Vec::new()
    } else {
        studio::draw(install, to_draw, survey::PICTURE_SIDE)?
    };
    let drawn_in = drawing.elapsed();
    for d in &drawn {
        if let Some(trouble) = &d.trouble {
            eprintln!("cairn: {}: {trouble}", d.install_path);
        }
    }
    let still = survey::pictures_missing(&inv, &order.dir).len();
    let pages = if still == 0 && order.draw_at_most.is_none() {
        Some(survey::write_pages(&inv, &order.dir)?)
    } else {
        None
    };
    let buildings = inv.models.iter().filter(|m| m.building).count();
    let mut said = format!(
        "cairn: {} models ({buildings} buildings), {} ground textures and {} zones read in {:.1} s\n\
         cairn: text, swatches and skies: wrote {}, kept {}\n\
         cairn: what fits where: wrote {tables} of {} tables{} in {:.1} s\n\
         cairn: drew {} pictures in {:.1} s; {still} still to draw",
        inv.models.len(),
        inv.grounds.len(),
        inv.zones.len(),
        read_in.as_secs_f32(),
        text.wrote,
        text.kept,
        fits::FILES.len(),
        if rules { " and the models' rules" } else { "" },
        counted_in.as_secs_f32(),
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
        install_path: m.path.clone(),
        building: m.building,
        bounds: m.bounds?,
        out: dir.join(format!("models/{}.png", m.key)),
    })
}

fn stamp_or_check(dir: &Path, data: &Path) -> Result<(), String> {
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
    let mut want = format!("{CATALOG_VERSION}\nthe install's archives:\n");
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

struct InstallLookups<'a> {
    install: &'a Install,
    areas: Areas,
    sounds: SoundTables,
}

impl Lookups for InstallLookups<'_> {
    fn zone_sound(&self, zone: &Zone) -> ZoneSound {
        self.sounds.of(zone)
    }

    fn zone_light(&self, zone: &Zone) -> Result<Option<u32>, String> {
        crate::zone::light_over_most_of(self.install, &self.areas, zone.area)
    }

    fn px_per_yard(&self, model: &Model) -> Option<f32> {
        let framing = studio::frame(model.bounds?);
        Some(survey::PICTURE_SIDE as f32 / framing.yards_across)
    }
}

struct SoundTables {
    areas: AreaSounds,
    kits: KitCatalog,
}

impl SoundTables {
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
