//! The commands that change a zone, typed, each with its text form: what the journal keeps and
//! what `cairn zone` takes. Every value is printed as it was typed, so a command read back from
//! its text is the same command.

mod scatter;

use std::fmt;

use crate::frame::{MAP_TILES, MAX_ZONE_TILES};
use crate::grammar::{Args, area, brush, check_word, join, point};
use crate::mask::{self, Mask, Span};
use crate::shape::Shape;
use crate::text::{number, parse_id};
use crate::zone::Id;

pub use scatter::{PerModel, Scatter};

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    New(New),
    Set(Set),
    Ground(Ground),
    Paint(Box<Paint>),
    Texture { path: String, effect: u32 },
    Place(Place),
    Move(Move),
    Remove(Vec<Id>),
    Scatter(Box<Scatter>),
    Relief(Relief),
    Water { level: f64, area: Shape },
    Dry(Vec<Id>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct New {
    pub tiles: (u32, u32),
    pub height: f64,
    pub texture: String,
    pub effect: u32,
    pub borrow: String,
    pub name: String,
    pub origin: (u32, u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Set {
    pub name: Option<String>,
    pub start: Option<[f64; 3]>,
    pub borrow: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GroundOp {
    Raise(f64),
    Lower(f64),
    /// Pull toward a height, or along a line toward the heights its points carry or the ground's
    /// there before the edit.
    Flatten {
        to: Option<f64>,
        strength: f64,
    },
    /// Pull toward the mean of each vertex's four neighbours, `passes` times.
    Smooth {
        strength: f64,
        passes: u32,
    },
    Roughen {
        amplitude: f64,
        feature_size: f64,
        seed: u64,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ground {
    pub op: GroundOp,
    pub brush: Shape,
    pub falloff: f64,
}

/// A stroke that moves a share of each texel it reaches to its texture, the chunk's other
/// textures giving that share up in proportion.
#[derive(Clone, Debug, PartialEq)]
pub struct Paint {
    pub texture: String,
    pub area: Shape,
    pub falloff: f64,
    pub strength: f64,
    /// Where a chunk already blends four textures, drop its least used other one first.
    pub make_room: bool,
    pub slope: Option<Span>,
    pub mask: Mask,
}

/// The relief of the install's zone `zone`, its ground less its own smoothed shape, laid onto an
/// area in blended patches.
#[derive(Clone, Debug, PartialEq)]
pub struct Relief {
    pub zone: String,
    pub area: Shape,
    pub falloff: f64,
    pub strength: f64,
    pub seed: u64,
}

/// How high a thing stands: above the ground under it, or at a world height, in yards.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Height {
    Above(f64),
    At(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub model: String,
    pub at: [f64; 2],
    pub facing: f64,
    pub scale: f64,
    pub z: Height,
    pub set: Option<u16>,
    pub lean: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Move {
    pub id: Id,
    pub to: Option<[f64; 2]>,
    pub facing: Option<f64>,
    pub turn: Option<f64>,
    pub scale: Option<f64>,
    pub z: Option<Height>,
    pub set: Option<u16>,
    pub lean: Option<bool>,
}

const BRUSH: [&str; 5] = ["at", "radius", "line", "width", "falloff"];
const AREA: [&str; 6] = ["at", "radius", "line", "width", "rect", "poly"];

impl Command {
    /// A command from its words, the verb first. A bare number names one of `author`'s own
    /// things.
    pub fn parse(words: &[String], author: &str) -> Result<Command, String> {
        for w in words {
            check_word(w)?;
        }
        let (verb, rest) = words.split_first().ok_or("no command")?;
        let a = Args::parse(rest);
        let with = |known: &[&[&str]]| a.allow_only(&known.concat());
        match verb.as_str() {
            "new" => {
                with(&[&[
                    "tiles", "height", "texture", "effect", "borrow", "name", "origin",
                ]])?;
                new(&a).map(Command::New)
            }
            "set" => {
                with(&[&["name", "start", "borrow"]])?;
                set(&a).map(Command::Set)
            }
            "raise" | "lower" | "flatten" | "smooth" | "roughen" => ground_command(verb, &a),
            "paint" => {
                with(&[
                    &AREA,
                    &["falloff", "strength", "make-room", "slope"],
                    &mask::FLAGS,
                ])?;
                let strength = a.num("strength")?.unwrap_or(1.0);
                if !(0.0..=1.0).contains(&strength) {
                    return Err("--strength is 0 to 1".into());
                }
                Ok(Command::Paint(Box::new(Paint {
                    texture: a
                        .first("which texture? `paint PATH --at X,Y --radius R`")?
                        .to_owned(),
                    area: area(&a)?,
                    falloff: falloff_of(&a)?,
                    strength,
                    make_room: a.has("make-room"),
                    slope: a
                        .one("slope")?
                        .map(|s| Span::parse(s, "--slope"))
                        .transpose()?,
                    mask: Mask::parse(&a)?,
                })))
            }
            "texture" => {
                with(&[&["effect"]])?;
                let effect = a.num("effect")?.ok_or(
                    "--effect N: its GroundEffectTexture id, 0 for none (the catalog's ground \
                     textures list the ones Blizzard's maps give it)",
                )?;
                Ok(Command::Texture {
                    path: a
                        .first("which texture? `texture PATH --effect N`")?
                        .to_owned(),
                    effect: effect as u32,
                })
            }
            "place" => {
                with(&[&["facing", "scale", "dz", "z", "set", "stands"]])?;
                place(&a).map(Command::Place)
            }
            "move" => {
                with(&[&["facing", "turn", "scale", "dz", "z", "set", "stands"]])?;
                shift(&a, author).map(Command::Move)
            }
            "remove" => {
                with(&[])?;
                ids(&a.pos, author, "which things? `remove ID ...`").map(Command::Remove)
            }
            "scatter" => {
                a.allow_only(&scatter::allowed())?;
                scatter::parse(&a).map(|s| Command::Scatter(Box::new(s)))
            }
            "relief" => {
                with(&[&AREA, &["falloff", "strength", "seed"]])?;
                relief(&a).map(Command::Relief)
            }
            "water" => {
                with(&[&AREA, &["remove"]])?;
                if let Some(v) = a.words("remove") {
                    return ids(v, author, "which water? `water --remove ID ...`")
                        .map(Command::Dry);
                }
                let level = a.first("at what level? `water 41.5 --poly X,Y ...`")?;
                Ok(Command::Water {
                    level: number(level, "water")?,
                    area: area(&a)?,
                })
            }
            v => Err(format!("{v:?} is not a command that changes a zone")),
        }
    }

    /// The command's words, the verb first: every value it holds, as typed.
    pub fn words(&self) -> Vec<String> {
        let mut w = Words::default();
        match self {
            Command::New(n) => {
                w.word("new");
                w.flag("tiles", [format!("{}x{}", n.tiles.0, n.tiles.1)]);
                w.num_flag("height", n.height);
                w.flag("texture", [n.texture.clone()]);
                w.flag("effect", [n.effect.to_string()]);
                w.flag("borrow", [n.borrow.clone()]);
                w.flag("name", [n.name.clone()]);
                w.flag("origin", [format!("{},{}", n.origin.0, n.origin.1)]);
            }
            Command::Set(s) => {
                w.word("set");
                if let Some(n) = &s.name {
                    w.flag("name", [n.clone()]);
                }
                if let Some([x, y, f]) = s.start {
                    w.flag("start", [format!("{x},{y},{f}")]);
                }
                if let Some(b) = &s.borrow {
                    w.flag("borrow", [b.clone()]);
                }
            }
            Command::Ground(g) => ground_words(&mut w, g),
            Command::Paint(p) => {
                w.word("paint");
                w.word(&p.texture);
                w.shape(&p.area);
                w.num_flag("falloff", p.falloff);
                w.num_flag("strength", p.strength);
                if p.make_room {
                    w.flag("make-room", []);
                }
                if let Some(s) = p.slope {
                    w.flag("slope", [s.text()]);
                }
                p.mask.words(&mut w);
            }
            Command::Texture { path, effect } => {
                w.word("texture");
                w.word(path);
                w.flag("effect", [effect.to_string()]);
            }
            Command::Place(p) => {
                w.word("place");
                w.word(&p.model);
                w.word(&format!("{},{}", p.at[0], p.at[1]));
                w.num_flag("facing", p.facing);
                w.num_flag("scale", p.scale);
                w.height(p.z);
                if let Some(s) = p.set {
                    w.flag("set", [s.to_string()]);
                }
                if p.lean {
                    w.flag("stands", [scatter::stands_text(true)]);
                }
            }
            Command::Move(m) => move_words(&mut w, m),
            Command::Remove(ids) => {
                w.word("remove");
                for id in ids {
                    w.word(&id.to_string());
                }
            }
            Command::Scatter(s) => scatter::words(&mut w, s),
            Command::Relief(r) => {
                w.word("relief");
                w.word(&r.zone);
                w.shape(&r.area);
                w.num_flag("falloff", r.falloff);
                w.num_flag("strength", r.strength);
                w.flag("seed", [r.seed.to_string()]);
            }
            Command::Water { level, area } => {
                w.word("water");
                w.word(&level.to_string());
                w.shape(area);
            }
            Command::Dry(ids) => {
                w.word("water");
                w.flag("remove", ids.iter().map(ToString::to_string));
            }
        }
        w.0
    }
}

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&join(&self.words()))
    }
}

#[derive(Default)]
pub(crate) struct Words(pub(crate) Vec<String>);

impl Words {
    fn word(&mut self, s: &str) {
        self.0.push(s.to_owned());
    }

    pub(crate) fn flag(&mut self, name: &str, values: impl IntoIterator<Item = String>) {
        self.0.push(format!("--{name}"));
        self.0.extend(values);
    }

    pub(crate) fn num_flag(&mut self, name: &str, v: f64) {
        self.flag(name, [v.to_string()]);
    }

    fn height(&mut self, z: Height) {
        match z {
            Height::Above(d) => self.num_flag("dz", d),
            Height::At(h) => self.num_flag("z", h),
        }
    }

    fn shape(&mut self, s: &Shape) {
        match s {
            Shape::Circle { c, r } => {
                self.flag("at", [format!("{},{}", c[0], c[1])]);
                self.num_flag("radius", *r);
            }
            Shape::Line { pts, z, width } => {
                let points = pts.iter().zip(z).map(|(p, z)| match z {
                    Some(z) => format!("{},{},{z}", p[0], p[1]),
                    None => format!("{},{}", p[0], p[1]),
                });
                self.flag("line", points);
                self.num_flag("width", *width);
            }
            Shape::Rect { a, b } => {
                self.flag(
                    "rect",
                    [format!("{},{}", a[0], a[1]), format!("{},{}", b[0], b[1])],
                );
            }
            Shape::Poly { pts } => {
                self.flag("poly", pts.iter().map(|p| format!("{},{}", p[0], p[1])));
            }
        }
    }
}

fn move_words(w: &mut Words, m: &Move) {
    w.word("move");
    w.word(&m.id.to_string());
    if let Some([x, y]) = m.to {
        w.word(&format!("{x},{y}"));
    }
    for (name, v) in [("facing", m.facing), ("turn", m.turn), ("scale", m.scale)] {
        if let Some(v) = v {
            w.num_flag(name, v);
        }
    }
    if let Some(z) = m.z {
        w.height(z);
    }
    if let Some(s) = m.set {
        w.flag("set", [s.to_string()]);
    }
    if let Some(l) = m.lean {
        w.flag("stands", [scatter::stands_text(l)]);
    }
}

fn ground_words(w: &mut Words, g: &Ground) {
    match &g.op {
        GroundOp::Raise(a) => {
            w.word("raise");
            w.word(&a.to_string());
        }
        GroundOp::Lower(a) => {
            w.word("lower");
            w.word(&a.to_string());
        }
        GroundOp::Flatten { to, .. } => {
            w.word("flatten");
            if let Some(t) = to {
                w.word(&t.to_string());
            }
        }
        GroundOp::Smooth { .. } => w.word("smooth"),
        GroundOp::Roughen { amplitude, .. } => {
            w.word("roughen");
            w.word(&amplitude.to_string());
        }
    }
    w.shape(&g.brush);
    w.num_flag("falloff", g.falloff);
    match &g.op {
        GroundOp::Flatten { strength, .. } => w.num_flag("strength", *strength),
        GroundOp::Smooth { strength, passes } => {
            w.num_flag("strength", *strength);
            w.flag("passes", [passes.to_string()]);
        }
        GroundOp::Roughen {
            feature_size, seed, ..
        } => {
            w.num_flag("size", *feature_size);
            w.flag("seed", [seed.to_string()]);
        }
        GroundOp::Raise(_) | GroundOp::Lower(_) => {}
    }
}

fn ground_command(verb: &str, a: &Args) -> Result<Command, String> {
    let with = |extra: &[&str]| a.allow_only(&[&BRUSH[..], extra].concat());
    let op = match verb {
        "raise" | "lower" => {
            with(&[])?;
            let amount = number(
                a.first("how many yards? `raise 5 --at X,Y --radius R`")?,
                verb,
            )?;
            if verb == "raise" {
                GroundOp::Raise(amount)
            } else {
                GroundOp::Lower(amount)
            }
        }
        "flatten" => {
            with(&["strength"])?;
            GroundOp::Flatten {
                to: a.pos.first().map(|s| number(s, "flatten")).transpose()?,
                strength: a.num("strength")?.unwrap_or(1.0),
            }
        }
        "smooth" => {
            with(&["strength", "passes"])?;
            GroundOp::Smooth {
                strength: a.num("strength")?.unwrap_or(1.0),
                passes: a.num("passes")?.unwrap_or(4.0) as u32,
            }
        }
        _ => {
            with(&["size", "seed"])?;
            let amplitude = number(
                a.first("how many yards? `roughen 2 --size 12 --at X,Y --radius R`")?,
                "roughen",
            )?;
            let feature_size = a.num("size")?.ok_or("--size S: the bumps' size in yards")?;
            if feature_size < 1.0 {
                return Err("--size is at least a yard".into());
            }
            GroundOp::Roughen {
                amplitude,
                feature_size,
                seed: a.num("seed")?.unwrap_or(0.0) as u64,
            }
        }
    };
    ground(a, op)
}

fn falloff_of(a: &Args) -> Result<f64, String> {
    let f = a.num("falloff")?.unwrap_or(0.5);
    if (0.0..=1.0).contains(&f) {
        Ok(f)
    } else {
        Err("--falloff is 0 (a hard edge) to 1 (easing from the middle)".into())
    }
}

fn ground(a: &Args, op: GroundOp) -> Result<Command, String> {
    let brush = brush(a)?;
    if let GroundOp::Flatten { to: None, .. } = op
        && !matches!(brush, Shape::Line { .. })
    {
        return Err(
            "flatten to what height? `flatten 42 --at X,Y --radius R`, or along a --line whose \
             points carry heights (X,Y,Z) or take the ground's"
                .into(),
        );
    }
    Ok(Command::Ground(Ground {
        op,
        brush,
        falloff: falloff_of(a)?,
    }))
}

fn words_of(a: &Args, flag: &str, want: &str) -> Result<Option<String>, String> {
    match a.words(flag) {
        None => Ok(None),
        Some([]) => Err(want.to_owned()),
        Some(w) => Ok(Some(w.join(" "))),
    }
}

fn new(a: &Args) -> Result<New, String> {
    let tiles = a
        .one("tiles")?
        .ok_or("--tiles WxH: how many tiles (533 yd each) east and south")?;
    let (w, h) = tiles.split_once('x').ok_or("--tiles WxH, such as 2x2")?;
    let tiles = (number(w, "--tiles")?, number(h, "--tiles")?);
    if !(1..=MAX_ZONE_TILES).contains(&tiles.0) || !(1..=MAX_ZONE_TILES).contains(&tiles.1) {
        return Err(format!("--tiles: 1 to {MAX_ZONE_TILES} each way"));
    }
    let origin = match a.one("origin")? {
        Some(s) => {
            let p = point(s)?;
            (p[0] as u32, p[1] as u32)
        }
        None => (32, 48),
    };
    if origin.0 + tiles.0 > MAP_TILES || origin.1 + tiles.1 > MAP_TILES {
        return Err("--origin: the zone must fit the 64×64 tiles of a map".into());
    }
    Ok(New {
        tiles,
        height: a
            .num("height")?
            .ok_or("--height H: the ground's height in yards")?,
        texture: a
            .one("texture")?
            .ok_or("--texture PATH: the ground's first texture")?
            .replace('/', "\\"),
        effect: a.num("effect")?.unwrap_or(0.0) as u32,
        borrow: words_of(a, "borrow", "--borrow ZONE")?.ok_or(
            "--borrow ZONE: the install's zone whose light, sky, water and music it takes, such \
             as --borrow Elwynn Forest",
        )?,
        name: words_of(a, "name", "--name NAME")?.ok_or("--name NAME")?,
        origin,
    })
}

fn set(a: &Args) -> Result<Set, String> {
    let start = match a.one("start")? {
        None => None,
        Some(s) => {
            let v = s
                .split(',')
                .map(|n| number(n, "--start"))
                .collect::<Result<Vec<f64>, _>>()?;
            match v[..] {
                [x, y] => Some([x, y, 0.0]),
                [x, y, f] => Some([x, y, f]),
                _ => return Err("--start X,Y[,FACING]".into()),
            }
        }
    };
    Ok(Set {
        name: words_of(a, "name", "--name NAME")?,
        start,
        borrow: words_of(
            a,
            "borrow",
            "--borrow ZONE: the install's zone whose sky and music it takes",
        )?,
    })
}

fn height_of(a: &Args) -> Result<Option<Height>, String> {
    match (a.num("dz")?, a.num("z")?) {
        (Some(_), Some(_)) => {
            Err("--dz (above the ground) or --z (a world height), not both".into())
        }
        (Some(d), None) => Ok(Some(Height::Above(d))),
        (None, Some(h)) => Ok(Some(Height::At(h))),
        (None, None) => Ok(None),
    }
}

fn set_of(a: &Args) -> Result<Option<u16>, String> {
    a.num("set").map(|s| s.map(|s| s as u16))
}

fn stands_of(a: &Args) -> Result<Option<bool>, String> {
    a.one("stands")?.map(scatter::stands).transpose()
}

fn place(a: &Args) -> Result<Place, String> {
    let model = a.first("which model? `place PATH X,Y`")?.to_owned();
    let at = point(a.pos.get(1).ok_or("where? `place PATH X,Y`")?)?;
    let building = model.to_ascii_lowercase().ends_with(".wmo");
    if !building && a.has("set") {
        return Err("--set is a building's doodad set".into());
    }
    let lean = stands_of(a)?.unwrap_or(false);
    if building && lean {
        return Err("a building stands upright".into());
    }
    Ok(Place {
        at,
        facing: a.num("facing")?.unwrap_or(0.0),
        scale: a.num("scale")?.unwrap_or(1.0),
        z: height_of(a)?.unwrap_or(Height::Above(0.0)),
        set: if building {
            Some(set_of(a)?.unwrap_or(0))
        } else {
            None
        },
        lean,
        model,
    })
}

fn shift(a: &Args, author: &str) -> Result<Move, String> {
    let id = id_or_own(a.first("which thing? `move ID [X,Y]`")?, author)?;
    Ok(Move {
        id,
        to: a.pos.get(1).map(|p| point(p)).transpose()?,
        facing: a.num("facing")?,
        turn: a.num("turn")?,
        scale: a.num("scale")?,
        z: height_of(a)?,
        set: set_of(a)?,
        lean: stands_of(a)?,
    })
}

fn relief(a: &Args) -> Result<Relief, String> {
    if a.pos.is_empty() {
        return Err("whose relief? `relief ZONE AREA`, the install's zone by name".into());
    }
    let strength = a.num("strength")?.unwrap_or(1.0);
    if !(0.0..=4.0).contains(&strength) {
        return Err("--strength is 0 to 4".into());
    }
    Ok(Relief {
        zone: a.pos.join(" "),
        area: area(a)?,
        falloff: falloff_of(a)?,
        strength,
        seed: a.num("seed")?.unwrap_or(0.0) as u64,
    })
}

/// An id, or a bare number naming one of `author`'s own.
fn id_or_own(s: &str, author: &str) -> Result<Id, String> {
    let bare = s.trim_start_matches('#');
    match bare.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Id {
            author: author.to_owned(),
            n,
        }),
        _ => parse_id(s),
    }
}

fn ids(words: &[String], author: &str, want: &str) -> Result<Vec<Id>, String> {
    if words.is_empty() {
        return Err(want.to_owned());
    }
    words.iter().map(|w| id_or_own(w, author)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::split;

    #[test]
    fn every_command_reads_back_from_its_text() {
        let lines = [
            "new --tiles 1x1 --height 48 --texture Tileset\\A.blp --effect 3515 --borrow Redridge Mountains --name Stone  Hollow --origin 30,40",
            "set --name \"Stone  'Hollow'\"",
            "set --start 336,440.5,340 --borrow Elwynn Forest",
            "raise 115 --line -20,25 553,25 --width 210 --falloff 0.55",
            "lower 20 --at 1.5,2 --radius 3",
            "flatten --line 15,305,125 70,314 --width 50 --falloff 0.9 --strength 0.7",
            "smooth --line 60,150 70,300 --width 130 --passes 10 --falloff 0.5",
            "roughen 3 --size 45 --at 280,320 --radius 170 --falloff 0.8 --seed 13",
            "paint \"Tileset\\Swamp of Sorrows\\A.blp\" --at 1,2 --radius 5 --make-room",
            "texture Tileset\\A.blp --effect 0",
            "place World\\wmo\\A.wmo 382,300 --facing 225 --z 39.8 --set 2",
            "move 12 372,297 --turn -45 --dz -0.4",
            "remove sam.1 ai.2 3",
            "scatter --models A.m2,B.m2 C.m2 --poly 12,135 150,148 165,258 --count 240 --apart 9 --scale 1.3..2.6 --seed 1",
            "scatter --models A.m2 B.m2 --rect 0,0 50,50 --count 9 --apart 2.6 30 --scale 0.8..1.2 1 --slope 0..47 3.. --stands upright leaning --water 1.5.. --off Tileset\\Road.blp --soft 2",
            "paint Tileset\\Rock.blp --at 200,200 --radius 250 --falloff 0 --slope 35..90 --height ..140 --on A.blp B.blp --soft 4",
            "relief Redridge Mountains --rect 0,0 533.33,533.33 --strength 0.5 --seed 2",
            "place A.m2 1,2 --stands leaning",
            "move 7 --stands upright",
            "water 40 --rect 291,84 315,99",
            "water --remove 7 ai.8",
        ];
        for line in lines {
            let c = Command::parse(&split(line).expect("words"), "sam").expect(line);
            let again = Command::parse(&split(&c.to_string()).expect("words"), "ai").expect(line);
            assert_eq!(c, again, "{line}\n{c}");
        }
    }

    #[test]
    fn a_command_says_what_it_wanted() {
        let err = |s: &str| Command::parse(&split(s).expect("words"), "sam").err();
        assert!(err("raise 5").is_some_and(|e| e.contains("--at")));
        assert!(err("place A.m2 1,2 --set 1").is_some_and(|e| e.contains("building")));
        assert!(err("flatten --at 1,1 --radius 2").is_some_and(|e| e.contains("what height")));
        assert!(err("paint A --at 1,1 --radius 2 --radius 3").is_some());
        assert!(err("jump").is_some());
        assert!(err("scatter --models A B C --at 1,1 --radius 9 --count 2 --apart 1 2").is_some());
        assert!(err("paint A --at 1,1 --radius 2 --slope 40..30").is_some());
        assert!(err("place A.wmo 1,1 --stands leaning").is_some());
        assert!(err("relief --at 1,1 --radius 9").is_some());
    }
}
