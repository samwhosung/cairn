use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use bevy::math::{UVec2, Vec3};
use world::TimeOfDay;

use crate::fixture::Fixture;
use crate::shot::DEFAULT_WORLD_AGE;
use crate::view::{HUMAN_START, Pose};

const DEFAULT_DISPLAY_AGE: f32 = 2.5;

pub const USAGE: &str = "\
usage: cairn [CAMERA] [--map MAP] [--time HH:MM] [--size WxH] [--no-glow] [--fly] [--mute] [LOOK]
             [--connect HOST:PORT | --host [PORT]] [--name NAME]
         walk the install at $WOW_DATA, starting where the camera looks, hearing it unless
         --mute keeps the window silent. The window serves its world to itself, and no one
         else can join it. --connect joins a running server, which places the player; --host
         also serves the world on 127.0.0.1:PORT (7777 by default), and each player who
         connects there appears beside the host. NAME is who the others see, the race's name
         by default
       cairn shot [CAMERA] [--map MAP] [--time HH:MM] [--size WxH] [--no-glow] [--age S]
                  --out FILE.png
         render one frame without a window, once everything in it has loaded and the
         world has run S seconds (2.5 by default)
       cairn shot --display ID [--age S] [--scale K] [--at X,Y,Z --az DEG --el DEG --dist YD] ...
         stand a CreatureDisplayInfo display on the ground below AT, K times its model's
         size (1 by default), and shoot it S seconds (2.5 by default) after it appears, from
         the orbit around the point a yard above its feet; without a camera, a Northshire
         hillside from 5 yd south, 10 degrees up

MAP is a Map.dbc id or directory name, Azeroth by default; --time is the game time
of day the world is lit for, 12:00 by default. --no-glow leaves out the client's
full-screen glow.

CAMERA, in WoW world coordinates (x north, y west, z up; yards and degrees):
  --eye X,Y,Z --look X,Y,Z                stand at the eye, look at the point
  --at X,Y,Z --az DEG --el DEG --dist YD  look at the point from DIST away: AZ 0 looks
                                          north, 90 west; EL is the height angle above it
Without one, the camera looks north over Northshire. --size defaults to 1600x900.

LOOK, the character walked as: --race human|orc|dwarf|nightelf|undead|tauren|gnome|troll
(or 1..8), --sex male|female, and --skin, --face, --hair, --hair-color, --facial-hair,
each counted from 0 as character creation offers them. A Human male by default, every
choice 0.

Walking: W and S run forward and back, A and D turn, Q and E strafe, Space jumps and
leaves the water, the wheel zooms to first person. A held left button turns the camera,
a held right button steers, both run. Num Lock runs on its own, keypad / walks.
Ctrl+Shift+F flies (--fly starts there): WASD moves, Space and C rise and sink, a held
button looks, the wheel sets the speed, Ctrl goes faster. Ctrl+Shift+G, flying, lands
where the camera is if the server lets the player teleport, as the window's own server
does; Ctrl+Shift+F again walks on from where the body stood.";

const FLAGS: [&str; 22] = [
    "age",
    "at",
    "az",
    "connect",
    "display",
    "dist",
    "el",
    "eye",
    "face",
    "facial-hair",
    "hair",
    "hair-color",
    "look",
    "map",
    "name",
    "out",
    "race",
    "scale",
    "sex",
    "size",
    "skin",
    "time",
];
/// The playable races by their `ChrRaces` id, 1 first.
const RACES: [&str; 8] = [
    "human", "orc", "dwarf", "nightelf", "undead", "tauren", "gnome", "troll",
];
const NORTHSHIRE_HILLSIDE: Vec3 = Vec3::new(-8960.0, -145.0, 90.0);
const DEFAULT_SIZE: UVec2 = UVec2::new(1600, 900);
const DEFAULT_MAP: &str = "Azeroth";
const NOON: TimeOfDay = TimeOfDay { minute: 12 * 60 };
const MAX_SIDE: u32 = 8192;
const DEFAULT_PORT: u16 = 7777;

#[derive(Debug, PartialEq)]
pub struct Args {
    pub pose: Pose,
    pub size: UVec2,
    pub map: String,
    pub time: TimeOfDay,
    pub mode: Mode,
    pub start_flying: bool,
    pub glow: bool,
    pub mute: bool,
    pub display: Option<Fixture>,
    pub world_age: Duration,
    pub look: Look,
    /// How the window joins a server; a shot joins none.
    pub join: Option<Joining>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joining {
    pub how: Join,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Alone,
    Connect(SocketAddr),
    Host(u16),
}

/// The character walked as: a `ChrRaces` id, 0 male or 1 female, and the five customization
/// choices, each an index into what character creation offers the race and sex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub race: u8,
    pub sex: u8,
    pub skin: u8,
    pub face: u8,
    pub hair: u8,
    pub hair_color: u8,
    pub facial_hair: u8,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            race: 1,
            sex: 0,
            skin: 0,
            face: 0,
            hair: 0,
            hair_color: 0,
            facial_hair: 0,
        }
    }
}

impl Look {
    /// The race's name as `--race` takes it.
    pub fn race_name(self) -> &'static str {
        RACES
            .get(usize::from(self.race).wrapping_sub(1))
            .copied()
            .unwrap_or("unknown")
    }

    fn race_title(self) -> String {
        let name = self.race_name();
        name[..1].to_ascii_uppercase() + &name[1..]
    }
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
    let mut glow = true;
    let mut mute = false;
    let mut host = None;
    while let Some(arg) = args.next() {
        if arg == "--fly" && !start_flying {
            start_flying = true;
            continue;
        }
        if arg == "--host" && host.is_none() {
            host = Some(host_port(args.next_if(|a| !a.starts_with("--")))?);
            continue;
        }
        if arg == "--no-glow" && glow {
            glow = false;
            continue;
        }
        if arg == "--mute" && !mute {
            mute = true;
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
    if shot && mute {
        return Err("--mute is for the window".into());
    }
    if let Some(path) = &out
        && !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", path.display()));
    }
    let display = display(&mut given, shot)?;
    let world_age = match given.remove("age") {
        Some(_) if !shot => return Err("--age is for a shot".into()),
        Some(age) => Duration::try_from_secs_f32(parse_number("age", &age)?)
            .map_err(|_| format!("--age wants seconds from 0, not {age}"))?,
        None => DEFAULT_WORLD_AGE,
    };
    let look = look(&mut given, shot)?;
    let join = join(&mut given, host, look, shot)?;
    Ok(Args {
        pose: pose(&given)?,
        size,
        map,
        time,
        mode: out.map_or(Mode::Window, Mode::Shot),
        start_flying,
        glow,
        mute,
        display,
        world_age,
        look,
        join,
    })
}

fn host_port(given: Option<String>) -> Result<u16, String> {
    given.map_or(Ok(DEFAULT_PORT), |p| {
        p.trim()
            .parse::<u16>()
            .map_err(|_| format!("--host wants a port, not {p}"))
    })
}

fn join(
    given: &mut BTreeMap<String, String>,
    host: Option<u16>,
    look: Look,
    shot: bool,
) -> Result<Option<Joining>, String> {
    let connect = given.remove("connect");
    if shot {
        return if connect.is_some() || host.is_some() {
            Err("--connect and --host are for the window".into())
        } else {
            Ok(None)
        };
    }
    let how = match (connect, host) {
        (Some(_), Some(_)) => {
            return Err("--connect and --host are two ways to join; give one".into());
        }
        (Some(addr), None) => addr
            .trim()
            .to_socket_addrs()
            .ok()
            .and_then(|mut found| found.next())
            .map(Join::Connect)
            .ok_or_else(|| format!("--connect wants HOST:PORT, not {addr}"))?,
        (None, Some(port)) => Join::Host(port),
        (None, None) => Join::Alone,
    };
    match (given.remove("name"), how) {
        (Some(_), Join::Alone) => Err("--name is for joining: --connect or --host".into()),
        (Some(name), _) if name.trim().is_empty() => Err("--name wants a name".into()),
        (name, how) => Ok(Some(Joining {
            how,
            name: name.map_or_else(|| look.race_title(), |n| n.trim().to_owned()),
        })),
    }
}

fn look(given: &mut BTreeMap<String, String>, shot: bool) -> Result<Look, String> {
    let mut look = Look::default();
    let flags = [
        "race",
        "sex",
        "skin",
        "face",
        "hair",
        "hair-color",
        "facial-hair",
    ];
    if shot && let Some(flag) = flags.iter().find(|f| given.contains_key(**f)) {
        return Err(format!("--{flag} is for the window"));
    }
    if let Some(race) = given.remove("race") {
        let name = race.trim().to_ascii_lowercase();
        look.race = match RACES.iter().position(|r| *r == name) {
            Some(i) => u8::try_from(i + 1).unwrap_or(1),
            None => name
                .parse::<u8>()
                .ok()
                .filter(|r| (1..=8).contains(r))
                .ok_or_else(|| {
                    format!(
                        "--race wants one of {} or 1..8, not {race}",
                        RACES.join(", ")
                    )
                })?,
        };
    }
    if let Some(sex) = given.remove("sex") {
        look.sex = match sex.trim().to_ascii_lowercase().as_str() {
            "male" | "0" => 0,
            "female" | "1" => 1,
            _ => return Err(format!("--sex wants male or female, not {sex}")),
        };
    }
    for (flag, dial) in [
        ("skin", &mut look.skin),
        ("face", &mut look.face),
        ("hair", &mut look.hair),
        ("hair-color", &mut look.hair_color),
        ("facial-hair", &mut look.facial_hair),
    ] {
        if let Some(value) = given.remove(flag) {
            *dial = value
                .trim()
                .parse::<u8>()
                .map_err(|_| format!("--{flag} wants a choice counted from 0, not {value}"))?;
        }
    }
    Ok(look)
}

fn display(given: &mut BTreeMap<String, String>, shot: bool) -> Result<Option<Fixture>, String> {
    let Some(id) = given.remove("display") else {
        return if given.contains_key("scale") {
            Err("--scale is for a display shot".into())
        } else {
            Ok(None)
        };
    };
    if !shot {
        return Err("--display is for a shot: cairn shot --display ID ...".into());
    }
    let display = id
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("--display wants a CreatureDisplayInfo id, not {id}"))?;
    let age = given
        .remove("age")
        .map_or(Ok(DEFAULT_DISPLAY_AGE), |a| parse_number("age", &a))?;
    if age < 0.0 {
        return Err("--age must not be negative".into());
    }
    let scale = given
        .remove("scale")
        .map_or(Ok(1.0), |k| parse_number("scale", &k))?;
    if scale <= 0.0 {
        return Err("--scale must be above 0".into());
    }
    let orbit = ["at", "az", "el", "dist"];
    let (at, az_deg, el_deg, dist) = if orbit.iter().all(|f| given.contains_key(*f)) {
        (
            parse_triple("at", &given["at"])?,
            parse_number("az", &given["az"])?,
            parse_number("el", &given["el"])?,
            parse_number("dist", &given["dist"])?,
        )
    } else if given.is_empty() {
        (NORTHSHIRE_HILLSIDE, 0.0, 10.0, 5.0)
    } else {
        return Err("a display shot takes its camera as --at, --az, --el and --dist".into());
    };
    if dist <= 0.0 {
        return Err("--dist must be above 0".into());
    }
    Ok(Some(Fixture {
        display,
        age,
        scale,
        at,
        az_deg,
        el_deg,
        dist,
    }))
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
    fn the_glow_is_on_unless_left_out() {
        assert!(parsed("").expect("parses").glow);
        assert!(!parsed("--no-glow --map 1").expect("parses").glow);
        assert!(!parsed("shot --no-glow --out a.png").expect("parses").glow);
        assert!(parsed("--no-glow --no-glow").is_err());
    }

    #[test]
    fn the_window_can_start_flying() {
        let args = parsed("--fly --map 1").expect("parses");
        assert!(args.start_flying && args.mode == Mode::Window);
        assert!(parsed("--fly --fly").is_err());
        assert!(parsed("shot --fly --out a.png").is_err());
    }

    #[test]
    fn the_window_can_be_muted() {
        assert!(!parsed("").expect("parses").mute);
        assert!(parsed("--mute --fly").expect("parses").mute);
        assert!(parsed("--mute --mute").is_err());
        assert!(parsed("shot --mute --out a.png").is_err());
    }

    #[test]
    fn the_walker_is_a_human_male_unless_told() {
        assert_eq!(parsed("").expect("parses").look, Look::default());
        let args = parsed(
            "--race Tauren --sex female --skin 3 --hair 2 --hair-color 1 --face 4 --facial-hair 5",
        )
        .expect("parses");
        assert_eq!(
            args.look,
            Look {
                race: 6,
                sex: 1,
                skin: 3,
                face: 4,
                hair: 2,
                hair_color: 1,
                facial_hair: 5,
            }
        );
        assert_eq!(args.look.race_name(), "tauren");
        assert_eq!(parsed("--race 8 --sex 0").expect("parses").look.race, 8);
    }

    #[test]
    fn a_window_joins_alone_by_address_or_hosting_on_a_port_as_its_race_unless_named() {
        let joining = |line: &str| parsed(line).expect("parses").join;
        let as_ = |how, name: &str| {
            Some(Joining {
                how,
                name: name.into(),
            })
        };
        assert_eq!(joining(""), as_(Join::Alone, "Human"));
        assert_eq!(joining("shot --out a.png"), None);
        let addr = SocketAddr::from(([127, 0, 0, 1], 7000));
        assert_eq!(
            joining("--connect 127.0.0.1:7000 --race orc"),
            as_(Join::Connect(addr), "Orc")
        );
        assert_eq!(
            joining("--host --name Brother"),
            as_(Join::Host(DEFAULT_PORT), "Brother")
        );
        assert_eq!(
            joining("--race 7 --host 7100"),
            as_(Join::Host(7100), "Gnome")
        );
    }

    #[test]
    fn a_shot_ages_its_world_two_and_a_half_seconds_unless_told() {
        let age = |line: &str| parsed(line).expect("parses").world_age;
        assert_eq!(age("shot --out a.png"), Duration::from_millis(2500));
        assert_eq!(age("shot --age 0 --out a.png"), Duration::ZERO);
        assert_eq!(age("shot --age 4 --out a.png"), Duration::from_secs(4));
    }

    #[test]
    fn a_display_shot_takes_its_subject_and_orbit() {
        let args = parsed("shot --display 3167 --age 2.5 --out a.png").expect("parses");
        assert_eq!(
            args.display,
            Some(Fixture {
                display: 3167,
                age: 2.5,
                scale: 1.0,
                at: NORTHSHIRE_HILLSIDE,
                az_deg: 0.0,
                el_deg: 10.0,
                dist: 5.0,
            })
        );
        let args = parsed(
            "shot --display 10913 --scale 1.35 --at 1,2,3 --az 90 --el 20 --dist 7 --out a.png",
        )
        .expect("parses");
        let f = args.display.expect("a display");
        assert_eq!(
            (f.age, f.scale, f.at, f.az_deg, f.el_deg, f.dist),
            (2.5, 1.35, Vec3::new(1.0, 2.0, 3.0), 90.0, 20.0, 7.0)
        );
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
            "--race elf",
            "--race 9",
            "--sex other",
            "--hair -1",
            "--skin 256",
            "shot --race orc --out a.png",
            "--display 3167",
            "--age 2",
            "shot --age -1 --out a.png",
            "shot --display x --out a.png",
            "shot --display 1 --age -1 --out a.png",
            "shot --scale 2 --out a.png",
            "shot --display 1 --scale 0 --out a.png",
            "shot --display 1 --at 0,0,0 --out a.png",
            "shot --display 1 --at 0,0,0 --az 0 --el 10 --dist 0 --out a.png",
            "--connect nowhere",
            "--connect 127.0.0.1:7000 --host",
            "--host 70000",
            "--host --host",
            "--name Anna",
            "shot --host --out a.png",
            "shot --connect 127.0.0.1:7000 --out a.png",
        ] {
            assert!(parsed(line).is_err(), "{line}");
        }
    }
}
