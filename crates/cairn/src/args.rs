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
             [--connect HOST:PORT | --host [PORT]] [--name NAME] [--world FILE]
             [--game NAME [--knobs FILE] [--overlay FILE]] [--notes DIR]
         walk the install at $WOW_DATA, starting where the camera looks, hearing it unless
         --mute keeps the window silent. The window serves its world to itself, and no one
         else can join it. --connect joins a running server, which places the player; --host
         also serves the world on 127.0.0.1:PORT (7777 by default), and each player who
         connects there appears beside the host. NAME is who the others see, the race's name
         by default. --game runs a game's rules (melee) on the window's own server, on its
         own knobs or the --knobs FILE, with an --overlay FILE laid on them; the window shows
         what the game has each body do, and takes where the game puts the player. --world
         keeps the window's own world in FILE and starts from it, the players known by name; a
         host keeps it by default in the user's data directory, named after its game or
         `world` (on macOS ~/Library/Application Support/cairn/worlds/NAME.sqlite), and a
         window alone keeps nothing unless told
       cairn shot [CAMERA] [--map MAP] [--time HH:MM] [--size WxH] [--no-glow] [--age S]
                  --out FILE.png
         render one frame without a window, once everything in it has loaded and the
         world has run S seconds (2.5 by default)
       cairn shot --display ID [--age S] [--scale K] [--at X,Y,Z --az DEG --el DEG --dist YD] ...
         stand a CreatureDisplayInfo display on the ground below AT, K times its model's
         size (1 by default), and shoot it S seconds (2.5 by default) after it appears, from
         the orbit around the point a yard above its feet; without a camera, a Northshire
         hillside from 5 yd south, 10 degrees up
       cairn atlas ZONE [--map MAP] [--yd N] [--mark X,Y]... --out FILE.png
         draw the AreaTable zone ZONE from above, north up, N yards a pixel (2 by default),
         on its own map unless MAP names another: the ground in its textures' colours, lit
         from the north-west and tinted by the water's depth, the land around the zone
         greyed, doodads as dots (trees dark green, shrubs light green, rocks grey, fences
         brown, props orange), buildings as red squares, and a ring at each point marked

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
a held right button steers, both run. Num Lock runs on its own, keypad / walks. The keys
1 to 0, - and = are WoW's first action bar: they send a game's actions 1 to 12.
Ctrl+Shift+F flies (--fly starts there): WASD moves, Space and C rise and sink, a held
button looks, the wheel sets the speed, Ctrl goes faster. Ctrl+Shift+G, flying, lands
where the camera is if the server lets the player teleport, as the window's own server
does; Ctrl+Shift+F again walks on from where the body stood.

Ctrl+Shift+N leaves a note about the spot under the pointer, or about the middle of the
window while a held button hides the pointer or it is off the window: a directory named
by its time (UTC) holding frame.png, the frame with the spot ringed, and note.txt, the
camera it was drawn from, what the spot shows, the shot that draws the view again and the
window that walks on from where it was taken. Notes go in the user's data directory (on
macOS ~/Library/Application Support/cairn/notes), or in --notes DIR.";

const FLAGS: [&str; 27] = [
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
    "game",
    "hair",
    "hair-color",
    "knobs",
    "look",
    "map",
    "name",
    "notes",
    "out",
    "overlay",
    "race",
    "scale",
    "sex",
    "size",
    "skin",
    "time",
    "world",
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
    pub notes: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Joining {
    pub how: Join,
    pub name: String,
    pub game: Option<GameChoice>,
    pub world: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameChoice {
    pub name: String,
    pub knobs: Option<PathBuf>,
    pub overlay: Option<PathBuf>,
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
    Window(Joining),
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
    let notes = given.remove("notes").map(PathBuf::from);
    if shot && notes.is_some() {
        return Err("--notes is for the window".into());
    }
    let mode = match out {
        Some(path) => {
            shot_joins_nothing(&given, host)?;
            Mode::Shot(path)
        }
        None => Mode::Window(join(&mut given, host, look)?),
    };
    Ok(Args {
        pose: pose(&given)?,
        size,
        map,
        time,
        mode,
        start_flying,
        glow,
        mute,
        display,
        world_age,
        look,
        notes,
    })
}

fn shot_joins_nothing(given: &BTreeMap<String, String>, host: Option<u16>) -> Result<(), String> {
    if given.contains_key("connect") || host.is_some() {
        return Err("--connect and --host are for the window".into());
    }
    if given.contains_key("name") {
        return Err("--name is for joining: --connect or --host".into());
    }
    if ["game", "knobs", "overlay", "world"]
        .iter()
        .any(|k| given.contains_key(*k))
    {
        return Err("--game, --knobs, --overlay and --world are for the window".into());
    }
    Ok(())
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
) -> Result<Joining, String> {
    let how = match (given.remove("connect"), host) {
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
    let game = match given.remove("game") {
        Some(_) if matches!(how, Join::Connect(_)) => {
            return Err("--game runs on the window's own server: alone or --host".into());
        }
        Some(name) => Some(GameChoice {
            name,
            knobs: given.remove("knobs").map(PathBuf::from),
            overlay: given.remove("overlay").map(PathBuf::from),
        }),
        None if given.contains_key("knobs") || given.contains_key("overlay") => {
            return Err("--knobs and --overlay are a game's: give --game".into());
        }
        None => None,
    };
    let world = match given.remove("world") {
        Some(_) if matches!(how, Join::Connect(_)) => {
            return Err("--world keeps the window's own world: alone or --host".into());
        }
        world => world.map(PathBuf::from),
    };
    match (given.remove("name"), how) {
        (Some(_), Join::Alone) => Err("--name is for joining: --connect or --host".into()),
        (Some(name), _) if name.trim().is_empty() => Err("--name wants a name".into()),
        (name, how) => Ok(Joining {
            how,
            name: name.map_or_else(|| look.race_title(), |n| n.trim().to_owned()),
            game,
            world,
        }),
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
mod tests;
