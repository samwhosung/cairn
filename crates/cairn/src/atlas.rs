use std::path::PathBuf;

use bevy::app::AppExit;
use world::{CurrentMap, Install};

use crate::args;

const DEFAULT_YARDS: f32 = 2.0;

#[derive(Debug, PartialEq)]
pub struct Order {
    pub drawn: Drawn,
    pub yards: f32,
    pub marks: Vec<[f32; 2]>,
    pub out: PathBuf,
}

#[derive(Debug, PartialEq)]
pub enum Drawn {
    Install {
        zone: String,
        map: Option<String>,
        patch: Option<PathBuf>,
    },
    Own(PathBuf),
}

pub fn main(argv: &[String]) -> AppExit {
    let order = match parse(argv.iter().cloned()) {
        Ok(order) => order,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", crate::args::USAGE);
            return AppExit::from_code(2);
        }
    };
    let opened = match &order.drawn {
        Drawn::Own(dir) => crate::open(&args::Map::Zone(dir.clone()))
            .map(|(install, map, _)| (install, Ok((map, None)))),
        Drawn::Install { zone, map, patch } => crate::install(patch.as_deref()).map(|install| {
            let found = install_zone(&install, zone, map.as_deref());
            (install, found)
        }),
    };
    let (install, found) = match opened {
        Ok(opened) => opened,
        Err(exit) => return exit,
    };
    match found.and_then(|(map, zone)| draw(&order, &install, &map, zone)) {
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
    let (mut zone, mut map, mut yards, mut patch, mut out) = (None, None, None, None, None);
    let mut dir = None;
    let mut marks = Vec::new();
    while let Some(arg) = args.next() {
        let Some(flag) = arg.strip_prefix("--") else {
            if let Some(first) = zone.replace(arg.clone()) {
                return Err(format!(
                    "one zone, quoted when its name has spaces, not {first} and {arg}"
                ));
            }
            continue;
        };
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match flag {
            "map" if map.is_none() => map = Some(value),
            "yd" if yards.is_none() => yards = Some(parse_yards(&value)?),
            "patch" if patch.is_none() => patch = Some(PathBuf::from(value)),
            "zone" if dir.is_none() => dir = Some(PathBuf::from(value)),
            "out" if out.is_none() => out = Some(PathBuf::from(value)),
            "mark" => marks.push(parse_point(&value)?),
            "map" | "yd" | "patch" | "zone" | "out" => {
                return Err(format!("{arg} is given twice"));
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let drawn = match (zone, dir) {
        (None, None) => return Err("name the zone: cairn atlas ZONE --out FILE.png".into()),
        (Some(_), Some(_)) => {
            return Err("--zone draws a zone of its own: name no zone of the install's".into());
        }
        (None, Some(_)) if map.is_some() || patch.is_some() => {
            return Err("--zone draws its own map, over the install as it is".into());
        }
        (None, Some(dir)) => Drawn::Own(dir),
        (Some(zone), None) => Drawn::Install { zone, map, patch },
    };
    let out = out.ok_or("an atlas needs --out FILE.png")?;
    if !out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", out.display()));
    }
    Ok(Order {
        drawn,
        yards: yards.unwrap_or(DEFAULT_YARDS),
        marks,
        out,
    })
}

fn install_zone(
    install: &Install,
    name: &str,
    on: Option<&str>,
) -> Result<(CurrentMap, Option<u32>), String> {
    let chain = &install.0;
    let areas = atlas::Areas::load(chain)?;
    let asked = on.map(|map| CurrentMap::find(chain, map)).transpose()?;
    let zone = areas
        .zone_named(name, asked.as_ref().map(|map| map.id))
        .ok_or_else(|| match &asked {
            Some(map) => format!("no zone on {} is named {name}", map.directory),
            None => format!("no zone is named {name}"),
        })?;
    let map = match asked {
        Some(map) => map,
        None => CurrentMap::find(chain, &areas.get(zone).map_or(0, |a| a.map).to_string())?,
    };
    Ok((map, Some(zone)))
}

fn parse_yards(value: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite() && *n > 0.0)
        .ok_or_else(|| format!("--yd wants yards a pixel above 0, not {value}"))
}

fn parse_point(value: &str) -> Result<[f32; 2], String> {
    let numbers: Option<Vec<f32>> = value
        .split(',')
        .map(|n| n.trim().parse::<f32>().ok().filter(|n| n.is_finite()))
        .collect();
    match numbers.as_deref() {
        Some(&[x, y]) => Ok([x, y]),
        _ => Err(format!("--mark wants X,Y, not {value}")),
    }
}

fn draw(
    order: &Order,
    install: &Install,
    map: &CurrentMap,
    zone: Option<u32>,
) -> Result<String, String> {
    let chain = &install.0;
    let areas = atlas::Areas::load(chain)?;
    let drawn = match &order.drawn {
        Drawn::Install { zone, .. } => format!("{zone} on {}", map.directory),
        Drawn::Own(_) => format!("{}, a zone of its own", map.directory),
    };
    let loaded = atlas::load_map(chain, &map.directory);
    let frame = atlas::frame(&loaded, &areas, zone).map_err(|_| match &order.drawn {
        Drawn::Install { zone, .. } => format!("{zone} has no terrain on {}", map.directory),
        Drawn::Own(_) => format!("no tile of {} reads", map.directory),
    })?;
    let colors = atlas::texture_colors(chain, &loaded);
    let (mut img, n) = atlas::render(&loaded, &colors, &areas, zone, &frame, order.yards)?;
    for &point in &order.marks {
        if !atlas::mark(&mut img, &frame, order.yards, point) {
            eprintln!("cairn: the mark {},{} is off the map", point[0], point[1]);
        }
    }
    img.save(&order.out)
        .map_err(|e| format!("writing {}: {e}", order.out.display()))?;
    let [x, y] = frame.corner();
    Ok(format!(
        "cairn: {drawn}, tiles {}..={} by {}..={}, {}x{} px at {} yd/px, north up, the top-left \
         corner at world {x:.1},{y:.1}\ncairn: standing in it, each once: {} trees, {} shrubs, \
         {} rocks, {} fences and walls, {} props; wrote {}",
        frame.x0,
        frame.x1,
        frame.y0,
        frame.y1,
        img.width(),
        img.height(),
        order.yards,
        n.trees,
        n.shrubs,
        n.rocks,
        n.fences,
        n.props,
        order.out.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Order, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn a_zone_is_drawn_two_yards_a_pixel_on_its_own_map_unless_told() {
        let order = parse(["Elwynn Forest", "--out", "a.png"].map(str::to_owned)).expect("parses");
        assert_eq!(
            order,
            Order {
                drawn: Drawn::Install {
                    zone: "Elwynn Forest".into(),
                    map: None,
                    patch: None,
                },
                yards: 2.0,
                marks: Vec::new(),
                out: PathBuf::from("a.png"),
            }
        );
        let order =
            parsed("--yd 0.5 --mark 1,-2 Tanaris --map 1 --patch p/q --mark -3.5,4 --out b/c.PNG")
                .expect("parses");
        let tanaris = Drawn::Install {
            zone: "Tanaris".into(),
            map: Some("1".into()),
            patch: Some(PathBuf::from("p/q")),
        };
        assert_eq!((order.drawn, order.yards), (tanaris, 0.5));
        assert_eq!(order.marks, vec![[1.0, -2.0], [-3.5, 4.0]]);
    }

    #[test]
    fn a_zone_of_its_own_is_drawn_whole_from_its_directory() {
        let order = parsed("--zone z/high --yd 1 --mark 1,2 --out a.png").expect("parses");
        let own = Drawn::Own(PathBuf::from("z/high"));
        assert_eq!((order.drawn, order.yards), (own, 1.0));
        for line in [
            "Westfall --zone z --out a.png",
            "--zone z --map 0 --out a.png",
            "--zone z --patch p --out a.png",
            "--zone z --zone y --out a.png",
            "--zone z",
        ] {
            assert!(parsed(line).is_err(), "{line}");
        }
    }

    #[test]
    fn mistakes_are_refused() {
        for line in [
            "",
            "Westfall",
            "--out a.png",
            "Elwynn Forest --out a.png",
            "Westfall --out a.jpg",
            "Westfall --out",
            "Westfall --yd 0 --out a.png",
            "Westfall --yd -1 --out a.png",
            "Westfall --yd inf --out a.png",
            "Westfall --yd two --out a.png",
            "Westfall --yd 1 --yd 2 --out a.png",
            "Westfall --map 0 --map 1 --out a.png",
            "Westfall --out a.png --out b.png",
            "Westfall --patch p --patch q --out a.png",
            "Westfall --mark 1 --out a.png",
            "Westfall --mark 1,2,3 --out a.png",
            "Westfall --mark 1,x --out a.png",
            "Westfall --size 64x64 --out a.png",
        ] {
            assert!(parsed(line).is_err(), "{line}");
        }
    }
}
