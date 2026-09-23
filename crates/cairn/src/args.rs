use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::math::{UVec2, Vec3};

use crate::view::{HUMAN_START, Pose};

pub const USAGE: &str = "\
usage: cairn [CAMERA] [--size WxH]
         fly a window over the install at $WOW_DATA
       cairn shot [CAMERA] [--size WxH] --out FILE.png
         render one frame without a window, once everything in it has loaded

CAMERA, in WoW world coordinates (x north, y west, z up; yards and degrees):
  --eye X,Y,Z --look X,Y,Z                stand at the eye, look at the point
  --at X,Y,Z --az DEG --el DEG --dist YD  look at the point from DIST away: AZ 0 looks
                                          north, 90 west; EL is the height angle above it
Without one, the camera looks north over Northshire. --size defaults to 1600x900.

In the window: WASD moves, Space and C rise and sink, a held mouse button looks,
the wheel sets the speed and Ctrl goes faster.";

const FLAGS: [&str; 8] = ["at", "az", "dist", "el", "eye", "look", "out", "size"];
const DEFAULT_SIZE: UVec2 = UVec2::new(1600, 900);
const MAX_SIDE: u32 = 8192;

#[derive(Debug, PartialEq)]
pub struct Args {
    pub pose: Pose,
    pub size: UVec2,
    pub mode: Mode,
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
    while let Some(arg) = args.next() {
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
    let out = given.remove("out").map(PathBuf::from);
    if out.is_some() != shot {
        return Err(if shot {
            "a shot needs --out FILE.png".into()
        } else {
            "--out is for a shot: cairn shot ...".into()
        });
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
        mode: out.map_or(Mode::Window, Mode::Shot),
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
    fn a_bare_command_flies_over_northshire() {
        let args = parsed("").expect("parses");
        assert_eq!(args.mode, Mode::Window);
        assert_eq!(args.size, DEFAULT_SIZE);
        assert_eq!(args.pose, Pose::orbit(HUMAN_START, 0.0, 12.0, 16.0));
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
        ] {
            assert!(parsed(line).is_err(), "{line}");
        }
    }
}
