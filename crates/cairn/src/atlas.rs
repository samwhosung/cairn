use std::path::PathBuf;

use bevy::app::AppExit;
use world::{CurrentMap, Install};

const DEFAULT_YARDS: f32 = 2.0;

#[derive(Debug, PartialEq)]
pub struct Order {
    pub zone: String,
    pub map: Option<String>,
    pub yards: f32,
    pub marks: Vec<[f32; 2]>,
    pub patch: Option<PathBuf>,
    pub out: PathBuf,
}

pub fn main(argv: &[String]) -> AppExit {
    let order = match parse(argv.iter().cloned()) {
        Ok(order) => order,
        Err(e) => {
            eprintln!("cairn: {e}\n\n{}", crate::args::USAGE);
            return AppExit::from_code(2);
        }
    };
    let install = match crate::install(order.patch.as_deref()) {
        Ok(install) => install,
        Err(exit) => return exit,
    };
    match draw(&order, &install) {
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
            "out" if out.is_none() => out = Some(PathBuf::from(value)),
            "mark" => marks.push(parse_point(&value)?),
            "map" | "yd" | "patch" | "out" => return Err(format!("{arg} is given twice")),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    let zone = zone.ok_or("name the zone: cairn atlas ZONE --out FILE.png")?;
    let out = out.ok_or("an atlas needs --out FILE.png")?;
    if !out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", out.display()));
    }
    Ok(Order {
        zone,
        map,
        yards: yards.unwrap_or(DEFAULT_YARDS),
        marks,
        patch,
        out,
    })
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

fn draw(order: &Order, install: &Install) -> Result<String, String> {
    let chain = &install.0;
    let areas = atlas::Areas::load(chain)?;
    let asked = order
        .map
        .as_deref()
        .map(|map| CurrentMap::find(chain, map))
        .transpose()?;
    let zone = areas
        .zone_named(&order.zone, asked.as_ref().map(|map| map.id))
        .ok_or_else(|| match &asked {
            Some(map) => format!("no zone on {} is named {}", map.directory, order.zone),
            None => format!("no zone is named {}", order.zone),
        })?;
    let map = match asked {
        Some(map) => map,
        None => CurrentMap::find(chain, &areas.get(zone).map_or(0, |a| a.map).to_string())?,
    };
    let loaded = atlas::load_map(chain, &map.directory);
    let frame = atlas::frame(&loaded, &areas, Some(zone))
        .map_err(|_| format!("{} has no terrain on {}", order.zone, map.directory))?;
    let colors = atlas::texture_colors(chain, &loaded);
    let (mut img, n) = atlas::render(&loaded, &colors, &areas, Some(zone), &frame, order.yards)?;
    for &point in &order.marks {
        if !atlas::mark(&mut img, &frame, order.yards, point) {
            eprintln!("cairn: the mark {},{} is off the map", point[0], point[1]);
        }
    }
    img.save(&order.out)
        .map_err(|e| format!("writing {}: {e}", order.out.display()))?;
    let [x, y] = frame.corner();
    Ok(format!(
        "cairn: {} on {}, tiles {}..={} by {}..={}, {}x{} px at {} yd/px, north up, the top-left \
         corner at world {x:.1},{y:.1}\ncairn: {} trees, {} shrubs, {} rocks, {} fences and \
         walls, {} props; wrote {}",
        order.zone,
        map.directory,
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
                zone: "Elwynn Forest".into(),
                map: None,
                yards: 2.0,
                marks: Vec::new(),
                patch: None,
                out: PathBuf::from("a.png"),
            }
        );
        let order =
            parsed("--yd 0.5 --mark 1,-2 Tanaris --map 1 --patch p/q --mark -3.5,4 --out b/c.PNG")
                .expect("parses");
        assert_eq!(
            (order.zone.as_str(), order.map.as_deref(), order.yards),
            ("Tanaris", Some("1"), 0.5)
        );
        assert_eq!(order.marks, vec![[1.0, -2.0], [-3.5, 4.0]]);
        assert_eq!(order.patch, Some(PathBuf::from("p/q")));
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
