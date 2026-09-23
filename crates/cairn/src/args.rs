use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::math::{UVec2, Vec3};
use world::TimeOfDay;

use crate::view::{HUMAN_START, Pose};

pub const USAGE: &str = "\
usage: cairn [CAMERA] [--map MAP] [--time HH:MM] [--size WxH] [--fly]
         walk the install at $WOW_DATA, starting where the camera looks
       cairn shot [CAMERA] [--map MAP] [--time HH:MM] [--size WxH] --out FILE.png
         render one frame without a window, once everything in it has loaded

MAP is a Map.dbc id or directory name, Azeroth by default; --time is the game time
of day the world is lit for, 12:00 by default.

CAMERA, in WoW world coordinates (x north, y west, z up; yards and degrees):
  --eye X,Y,Z --look X,Y,Z                stand at the eye, look at the point
  --at X,Y,Z --az DEG --el DEG --dist YD  look at the point from DIST away: AZ 0 looks
                                          north, 90 west; EL is the height angle above it
Without one, the camera looks north over Northshire. --size defaults to 1600x900.

Walking: W and S run forward and back, A and D turn, Q and E strafe, Space jumps and
leaves the water, the wheel zooms to first person. A held left button turns the camera,
a held right button steers, both run. Num Lock runs on its own, keypad / walks.
Ctrl+Shift+F flies (--fly starts there): WASD moves, Space and C rise and sink, a held
button looks, the wheel sets the speed, Ctrl goes faster. Ctrl+Shift+G, flying, lands
where the camera is; Ctrl+Shift+F again walks on from where the body stood.";

const FLAGS: [&str; 10] = [
    "at", "az", "dist", "el", "eye", "look", "map", "out", "size", "time",
];
const DEFAULT_SIZE: UVec2 = UVec2::new(1600, 900);
const DEFAULT_MAP: &str = "Azeroth";
const NOON: TimeOfDay = TimeOfDay { minute: 12 * 60 };
const MAX_SIDE: u32 = 8192;

#[derive(Debug, PartialEq)]
pub struct Args {
    pub pose: Pose,
    pub size: UVec2,
    pub map: String,
    pub time: TimeOfDay,
    pub mode: Mode,
    pub start_flying: bool,
}

#[derive(Debug, PartialEq)]
pub enum Mode {
    Window,
    Shot(PathBuf),
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut args = args.into_iter().peekable();
    let shot = args.next_if(|arg| arg == "shot").is_some();
    let mut given = BTreeMap::new();
    let mut start_flying = false;
    while let Some(arg) = args.next() {
        if arg == "--fly" && !start_flying {
            start_flying = true;
            continue;
        }
        let flag = arg
            .strip_prefix("--")
            .filter(|flag| FLAGS.contains(flag))
            .ok_or_else(|| format!("unknown argument {arg}"))?;
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        if given.insert(flag.to_owned(), value).is_some() {
            return Err(format!("{arg} is given twice"));
        }
    }
    let size = given
        .remove("size")
        .map_or(Ok(DEFAULT_SIZE), |size| parse_size(&size))?;
    let map = given
        .remove("map")
        .unwrap_or_else(|| DEFAULT_MAP.to_owned());
    let time = given
        .remove("time")
        .map_or(Ok(NOON), |time| parse_time(&time))?;
    let out = given.remove("out").map(PathBuf::from);
    if out.is_some() != shot {
        return Err(if shot {
            "a shot needs --out FILE.png".into()
        } else {
            "--out is for a shot: cairn shot ...".into()
        });
    }
    if shot && start_flying {
        return Err("--fly is for the window".into());
    }
    if let Some(path) = &out
        && !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", path.display()));
    }
    Ok(Args {
        pose: pose(&given)?,
        size,
        map,
        time,
        mode: out.map_or(Mode::Window, Mode::Shot),
        start_flying,
    })
}

fn pose(given: &BTreeMap<String, String>) -> Result<Pose, String> {
    let triple = |flag: &str| parse_triple(flag, &given[flag]);
    let number = |flag: &str| parse_number(flag, &given[flag]);
    match given.keys().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => Ok(Pose::orbit(HUMAN_START, 0.0, 12.0, 16.0)),
        ["eye", "look"] => {
            let (eye, look) = (triple("eye")?, triple("look")?);
            if eye == look {
                return Err("--eye and --look are the same point".into());
            }
            Ok(Pose::look(eye, look))
        }
        ["at", "az", "dist", "el"] => {
            let (dist, el) = (number("dist")?, number("el")?);
            if dist <= 0.0 {
                return Err("--dist must be above 0".into());
            }
            if el.abs() > 90.0 {
                return Err("--el must lie within -90..90".into());
            }
            Ok(Pose::orbit(triple("at")?, number("az")?, el, dist))
        }
        _ => Err("give the camera as --eye and --look, or as --at, --az, --el and --dist".into()),
    }
}

fn parse_number(flag: &str, value: &str) -> Result<f32, String> {
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("--{flag} wants a number, not {value}"))
}

fn parse_triple(flag: &str, value: &str) -> Result<Vec3, String> {
    let numbers: Vec<f32> = value
        .split(',')
        .map(|part| parse_number(flag, part))
        .collect::<Result<_, _>>()?;
    let [x, y, z] = numbers[..] else {
        return Err(format!("--{flag} wants X,Y,Z, not {value}"));
    };
    Ok(Vec3::new(x, y, z))
}

fn parse_time(value: &str) -> Result<TimeOfDay, String> {
    let field = |s: &str, below: u32| s.parse::<u32>().ok().filter(|n| *n < below);
    value
        .split_once(':')
        .and_then(|(h, m)| Some(field(h, 24)? * 60 + field(m, 60)?))
        .map(|minute| TimeOfDay { minute })
        .ok_or_else(|| format!("--time wants HH:MM, 00:00 to 23:59, not {value}"))
}

fn parse_size(value: &str) -> Result<UVec2, String> {
    let side = |s: &str| s.parse::<u32>().ok().filter(|n| (1..=MAX_SIDE).contains(n));
    value
        .split_once('x')
        .and_then(|(w, h)| Some(UVec2::new(side(w)?, side(h)?)))
        .ok_or_else(|| format!("--size wants WxH, each 1..{MAX_SIDE}, not {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Args, String> {
        parse(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn a_bare_command_walks_northshire() {
        let args = parsed("").expect("parses");
        assert_eq!(args.mode, Mode::Window);
        assert!(!args.start_flying);
        assert_eq!(args.pose.target, HUMAN_START);
        assert_eq!(args.size, DEFAULT_SIZE);
        assert_eq!(args.pose, Pose::orbit(HUMAN_START, 0.0, 12.0, 16.0));
        assert_eq!((args.map.as_str(), args.time.minute), ("Azeroth", 720));
    }

    #[test]
    fn the_map_and_the_hour_are_taken_as_given() {
        let args = parsed("shot --map 1 --time 06:30 --out a.png").expect("parses");
        assert_eq!((args.map.as_str(), args.time.minute), ("1", 390));
        let args = parsed("--time 23:59 --map Kalimdor").expect("parses");
        assert_eq!((args.map.as_str(), args.time.minute), ("Kalimdor", 1439));
    }

    #[test]
    fn a_shot_takes_either_camera_form() {
        let orbit = parsed("shot --at 1,2,3 --az 90 --el 30 --dist 10 --out a/b.png --size 64x32")
            .expect("parses");
        assert_eq!(
            orbit.pose,
            Pose::orbit(Vec3::new(1.0, 2.0, 3.0), 90.0, 30.0, 10.0)
        );
        assert_eq!(orbit.size, UVec2::new(64, 32));
        assert_eq!(orbit.mode, Mode::Shot(PathBuf::from("a/b.png")));
        let look = parsed("shot --out x.PNG --look 1,0,0 --eye 0,0,0").expect("parses");
        assert_eq!(look.pose, Pose::look(Vec3::ZERO, Vec3::X));
    }

    #[test]
    fn the_window_can_start_flying() {
        let args = parsed("--fly --map 1").expect("parses");
        assert!(args.start_flying && args.mode == Mode::Window);
        assert!(parsed("--fly --fly").is_err());
        assert!(parsed("shot --fly --out a.png").is_err());
    }

    #[test]
    fn mistakes_are_refused() {
        for line in [
            "shot",
            "--out a.png",
            "shot --out a.jpg",
            "--eye 0,0,0",
            "--eye 0,0,0 --look 1,0,0 --az 3",
            "--eye 1,2,3 --look 1,2,3",
            "--eye 0,0 --look 1,0,0",
            "--at 0,0,0 --az 0 --el 91 --dist 5",
            "--at 0,0,0 --az 0 --el 10 --dist 0",
            "--at 0,0,0 --az north --el 10 --dist 5",
            "--size 1600",
            "--size 0x900",
            "--size 1600x900 --size 800x600",
            "--fov 90",
            "--eye",
            "--time 24:00",
            "--time 12:60",
            "--time 1230",
            "--time noon",
        ] {
            assert!(parsed(line).is_err(), "{line}");
        }
    }
}
