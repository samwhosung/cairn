use bevy::math::{UVec2, Vec2, Vec3};

use crate::args::{parse_number, parse_triple};

#[derive(Debug, PartialEq)]
pub enum HandsAsk {
    Pick { at: UVec2, ground: bool },
    Select(Vec<u32>),
    Pointer(UVec2),
    Ghost(Option<GhostAsk>),
    Add { id: u32, model: Model, at: Spot },
    Move(Move),
    Remove(Vec<u32>),
}

#[derive(Debug, PartialEq)]
pub struct Model {
    pub file: String,
    pub facing_deg: f32,
    pub scale: Option<f32>,
    pub set: u16,
}

#[derive(Debug, PartialEq)]
pub struct GhostAsk {
    pub model: Model,
    pub at: Option<Spot>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    Ground(Vec2),
    At(Vec3),
}

#[derive(Debug, PartialEq)]
pub struct Move {
    pub id: u32,
    pub to: Option<Spot>,
    pub by: Option<Vec3>,
    pub turn: Turn,
    pub scale: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Turn {
    Keep,
    Facing(f32),
    By(f32),
}

pub fn parse(verb: &str, said: &[&str]) -> Result<HandsAsk, String> {
    match (verb, said) {
        ("pick", [x, y]) => Ok(HandsAsk::Pick {
            at: pixel(x, y)?,
            ground: false,
        }),
        ("pick", [x, y, "--ground"]) => Ok(HandsAsk::Pick {
            at: pixel(x, y)?,
            ground: true,
        }),
        ("pick", _) => {
            Err("pick wants a pixel, from the frame's top left: pick X Y [--ground]".into())
        }
        ("select", ids) => ids
            .iter()
            .map(|id| unique_id(id))
            .collect::<Result<_, _>>()
            .map(HandsAsk::Select),
        ("pointer", [x, y]) => Ok(HandsAsk::Pointer(pixel(x, y)?)),
        ("pointer", _) => {
            Err("pointer wants a pixel, from the frame's top left: pointer X Y".into())
        }
        ("ghost", []) => Ok(HandsAsk::Ghost(None)),
        ("ghost", [file, rest @ ..]) => {
            let (at, flags) = match rest.split_first() {
                Some((at, flags)) if !at.starts_with("--") => (Some(spot(at)?), flags),
                _ => (None, rest),
            };
            let mut flags = Flags::of(flags)?;
            if flags.take("set").is_some() {
                return Err("a ghost draws no building's doodads: --set is for add".into());
            }
            let model = model(file, &mut flags)?;
            flags.done()?;
            Ok(HandsAsk::Ghost(Some(GhostAsk { model, at })))
        }
        ("add", [id, file, at, flags @ ..]) => {
            let mut flags = Flags::of(flags)?;
            let model = model(file, &mut flags)?;
            flags.done()?;
            Ok(HandsAsk::Add {
                id: unique_id(id)?,
                model,
                at: spot(at)?,
            })
        }
        ("add", _) => Err("add wants ID FILE X,Y[,Z]".into()),
        ("move", [id, rest @ ..]) => {
            let (to, flags) = match rest.split_first() {
                Some((at, flags)) if !at.starts_with("--") => (Some(spot(at)?), flags),
                _ => (None, rest),
            };
            let mut flags = Flags::of(flags)?;
            let by = flags
                .take("by")
                .map(|v| parse_triple("by", v))
                .transpose()?;
            let facing = flags.number("facing")?;
            let turn = flags.number("turn")?;
            let scale = flags.number("scale")?;
            flags.done()?;
            let turn = match (facing, turn) {
                (Some(_), Some(_)) => {
                    return Err("--facing and --turn are two ways to turn; give one".into());
                }
                (Some(f), None) => Turn::Facing(f),
                (None, Some(t)) => Turn::By(t),
                (None, None) => Turn::Keep,
            };
            if to.is_none() && by.is_none() && turn == Turn::Keep && scale.is_none() {
                return Err("move ID wants a place, --by, --facing, --turn or --scale".into());
            }
            Ok(HandsAsk::Move(Move {
                id: unique_id(id)?,
                to,
                by,
                turn,
                scale: scale.map(positive_scale).transpose()?,
            }))
        }
        ("remove", ids) if !ids.is_empty() => ids
            .iter()
            .map(|id| unique_id(id))
            .collect::<Result<_, _>>()
            .map(HandsAsk::Remove),
        ("remove", _) => Err("remove wants one unique id or more".into()),
        (verb, _) => Err(format!("{verb} takes something else")),
    }
}

fn pixel(x: &str, y: &str) -> Result<UVec2, String> {
    let side = |v: &str| {
        v.trim()
            .parse::<u32>()
            .map_err(|_| format!("a pixel is counted in whole pixels from 0, not {v}"))
    };
    Ok(UVec2::new(side(x)?, side(y)?))
}

fn unique_id(v: &str) -> Result<u32, String> {
    v.trim()
        .parse::<u32>()
        .map_err(|_| format!("a unique id is a whole number, not {v}"))
}

fn spot(v: &str) -> Result<Spot, String> {
    let numbers: Vec<f32> = v
        .split(',')
        .map(|n| parse_number("place", n))
        .collect::<Result<_, _>>()?;
    match numbers[..] {
        [x, y] => Ok(Spot::Ground(Vec2::new(x, y))),
        [x, y, z] => Ok(Spot::At(Vec3::new(x, y, z))),
        _ => Err(format!("a place is X,Y on the ground or X,Y,Z, not {v}")),
    }
}

fn positive_scale(s: f32) -> Result<f32, String> {
    if s > 0.0 {
        Ok(s)
    } else {
        Err(format!("--scale wants a size above 0, not {s}"))
    }
}

fn model(file: &str, flags: &mut Flags<'_>) -> Result<Model, String> {
    let set = match flags.take("set") {
        Some(v) => v
            .trim()
            .parse::<u16>()
            .map_err(|_| format!("--set wants a doodad set's index, not {v}"))?,
        None => 0,
    };
    Ok(Model {
        file: file.to_owned(),
        facing_deg: flags.number("facing")?.unwrap_or(0.0),
        scale: flags.number("scale")?.map(positive_scale).transpose()?,
        set,
    })
}

struct Flags<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Flags<'a> {
    fn of(words: &[&'a str]) -> Result<Self, String> {
        let mut out = Vec::new();
        let mut words = words.iter();
        while let Some(&word) = words.next() {
            let flag = word
                .strip_prefix("--")
                .filter(|f| ["by", "facing", "turn", "scale", "set"].contains(f))
                .ok_or_else(|| format!("{word} is not a flag here"))?;
            let value = words
                .next()
                .ok_or_else(|| format!("{word} needs a value"))?;
            if out.iter().any(|(f, _)| *f == flag) {
                return Err(format!("{word} is given twice"));
            }
            out.push((flag, *value));
        }
        Ok(Self(out))
    }

    fn take(&mut self, flag: &str) -> Option<&'a str> {
        let at = self.0.iter().position(|(f, _)| *f == flag);
        at.map(|i| self.0.remove(i).1)
    }

    fn number(&mut self, flag: &str) -> Result<Option<f32>, String> {
        self.take(flag).map(|v| parse_number(flag, v)).transpose()
    }

    fn done(&self) -> Result<(), String> {
        match self.0.first() {
            Some((flag, _)) => Err(format!("--{flag} is not for this command")),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_take_pixels_ids_places_and_flags() {
        assert_eq!(
            parse("pick", &["3", "4", "--ground"]),
            Ok(HandsAsk::Pick {
                at: UVec2::new(3, 4),
                ground: true
            })
        );
        assert!(parse("pick", &["3.5", "4"]).is_err());
        assert_eq!(parse("select", &[]), Ok(HandsAsk::Select(Vec::new())));
        assert_eq!(
            parse(
                "add",
                &[
                    "7",
                    "World\\Lamp.m2",
                    "1,2",
                    "--facing",
                    "90",
                    "--scale",
                    "2"
                ]
            ),
            Ok(HandsAsk::Add {
                id: 7,
                model: Model {
                    file: "World\\Lamp.m2".into(),
                    facing_deg: 90.0,
                    scale: Some(2.0),
                    set: 0
                },
                at: Spot::Ground(Vec2::new(1.0, 2.0)),
            })
        );
        assert_eq!(
            parse("move", &["7", "--by", "0,-10,0"]),
            Ok(HandsAsk::Move(Move {
                id: 7,
                to: None,
                by: Some(Vec3::new(0.0, -10.0, 0.0)),
                turn: Turn::Keep,
                scale: None,
            }))
        );
        assert_eq!(
            parse("move", &["7", "1,2,3", "--turn", "45"]),
            Ok(HandsAsk::Move(Move {
                id: 7,
                to: Some(Spot::At(Vec3::new(1.0, 2.0, 3.0))),
                by: None,
                turn: Turn::By(45.0),
                scale: None,
            }))
        );
        assert!(parse("move", &["7"]).is_err(), "a move says what changes");
        assert!(parse("move", &["7", "--facing", "1", "--turn", "2"]).is_err());
        assert!(parse("add", &["7", "a.m2", "1,2", "--by", "1,1,1"]).is_err());
        assert!(parse("ghost", &["a.m2", "--scale", "0"]).is_err());
        assert!(parse("ghost", &["a.wmo", "--set", "1"]).is_err());
        let ghost = parse("ghost", &["a.m2", "1,2,3", "--facing", "9"]);
        let Ok(HandsAsk::Ghost(Some(GhostAsk { model, at }))) = ghost else {
            panic!("a ghost at a place: {ghost:?}");
        };
        assert_eq!(
            (model.facing_deg, at),
            (9.0, Some(Spot::At(Vec3::new(1.0, 2.0, 3.0))))
        );
        assert_eq!(
            parse("remove", &["1", "2"]),
            Ok(HandsAsk::Remove(vec![1, 2]))
        );
    }
}
