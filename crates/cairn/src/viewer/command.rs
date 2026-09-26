use std::path::PathBuf;

use bevy::math::{UVec2, Vec3};

use crate::args::{parse_aim, parse_number, parse_size, parse_triple};
use crate::view::Aim;

#[derive(Debug, PartialEq)]
pub enum Command {
    /// A blank line or a comment.
    Nothing,
    Look(Aim),
    /// Yards forward along the view, to its left and straight up.
    Move {
        forward: f32,
        left: f32,
        up: f32,
    },
    /// Degrees to the left and up, standing where it stands.
    Turn {
        left: f32,
        up: f32,
    },
    /// Degrees round the point it looks at, to its own left and up, and yards closer to it.
    Orbit {
        left: f32,
        up: f32,
        closer: f32,
    },
    Shot(Ask),
    Where,
    Quit,
}

#[derive(Debug, PartialEq)]
pub struct Ask {
    pub out: PathBuf,
    pub size: Option<UVec2>,
    pub cut_to: Option<Vec3>,
    pub cut_near: Option<f32>,
    pub leave_out: Vec<u32>,
    pub seen: bool,
}

impl Ask {
    /// Whether it leaves anything out.
    pub fn cuts(&self) -> bool {
        self.cut_to.is_some() || self.cut_near.is_some() || !self.leave_out.is_empty()
    }
}

type Ways<'a> = &'a [(&'a str, usize, f32)];

const MOVES: Ways<'static> = &[
    ("forward", 0, 1.0),
    ("back", 0, -1.0),
    ("left", 1, 1.0),
    ("right", 1, -1.0),
    ("up", 2, 1.0),
    ("down", 2, -1.0),
];
const TURNS: Ways<'static> = &[
    ("left", 0, 1.0),
    ("right", 0, -1.0),
    ("up", 1, 1.0),
    ("down", 1, -1.0),
];
const ORBITS: Ways<'static> = &[
    ("left", 0, 1.0),
    ("right", 0, -1.0),
    ("up", 1, 1.0),
    ("down", 1, -1.0),
    ("in", 2, 1.0),
    ("out", 2, -1.0),
];

pub fn parse(line: &str) -> Result<Command, String> {
    let words = split(line)?;
    let Some((verb, rest)) = words.split_first() else {
        return Ok(Command::Nothing);
    };
    let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
    match (verb.as_str(), &rest[..]) {
        (verb, _) if verb.starts_with('#') => Ok(Command::Nothing),
        ("look", flags) => parse_aim(flags).map(Command::Look),
        ("move", said) => {
            let [forward, left, up] = amounts(said, MOVES)?;
            Ok(Command::Move { forward, left, up })
        }
        ("turn", said) => {
            let [left, up] = amounts(said, TURNS)?;
            Ok(Command::Turn { left, up })
        }
        ("orbit", said) => {
            let [left, up, closer] = amounts(said, ORBITS)?;
            Ok(Command::Orbit { left, up, closer })
        }
        ("shot", said) => shot(said).map(Command::Shot),
        ("where", []) => Ok(Command::Where),
        ("quit", []) => Ok(Command::Quit),
        (verb @ ("where" | "quit"), _) => Err(format!("{verb} takes nothing more")),
        (verb, _) => Err(format!(
            "no command {verb}: look, move, turn, orbit, shot, where or quit"
        )),
    }
}

/// Sums what each way is given into its slot, as `left 30 up 5`.
fn amounts<const N: usize>(said: &[&str], ways: Ways<'_>) -> Result<[f32; N], String> {
    let names = || {
        ways.iter()
            .map(|(name, ..)| *name)
            .collect::<Vec<_>>()
            .join(", ")
    };
    if said.is_empty() || !said.len().is_multiple_of(2) {
        return Err(format!(
            "want a way and an amount, one or more: {}",
            names()
        ));
    }
    let mut out = [0.0; N];
    let mut seen = Vec::new();
    for pair in said.chunks(2) {
        let (way, amount) = (pair[0], pair[1]);
        let &(name, slot, sign) = ways
            .iter()
            .find(|(name, ..)| *name == way)
            .ok_or_else(|| format!("no way {way}: {}", names()))?;
        if seen.contains(&slot) {
            return Err(format!(
                "{name} and its opposite are given together, or twice"
            ));
        }
        seen.push(slot);
        out[slot] = sign * parse_number(name, amount)?;
    }
    Ok(out)
}

fn shot(said: &[&str]) -> Result<Ask, String> {
    let Some((out, flags)) = said.split_first() else {
        return Err("shot wants a file: shot FILE.png".into());
    };
    let out = PathBuf::from(out);
    if !out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("png"))
    {
        return Err(format!("{} does not end in .png", out.display()));
    }
    let mut ask = Ask {
        out,
        size: None,
        cut_to: None,
        cut_near: None,
        leave_out: Vec::new(),
        seen: false,
    };
    let mut flags = flags.iter();
    while let Some(&flag) = flags.next() {
        if flag == "--seen" && !ask.seen {
            ask.seen = true;
            continue;
        }
        let mut value = || {
            flags
                .next()
                .copied()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--size" if ask.size.is_none() => ask.size = Some(parse_size(value()?)?),
            "--cut-to" if ask.cut_to.is_none() => {
                ask.cut_to = Some(parse_triple("cut-to", value()?)?);
            }
            "--cut-near" if ask.cut_near.is_none() => {
                let yards = parse_number("cut-near", value()?)?;
                if yards <= 0.0 {
                    return Err("--cut-near wants yards above 0".into());
                }
                ask.cut_near = Some(yards);
            }
            "--leave-out" if ask.leave_out.is_empty() => {
                let ids = value()?;
                ask.leave_out = ids
                    .split(',')
                    .map(|id| id.trim().parse::<u32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| format!("--leave-out wants unique ids, as 12,57, not {ids}"))?;
            }
            "--size" | "--cut-to" | "--cut-near" | "--leave-out" | "--seen" => {
                return Err(format!("{flag} is given twice"));
            }
            _ => {
                return Err(format!(
                    "a shot takes --size, --cut-to, --cut-near, --leave-out and --seen, not {flag}"
                ));
            }
        }
    }
    Ok(ask)
}

/// Words split at white space, a quoted word whole.
fn split(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                word.get_or_insert_default();
            }
            None if c.is_whitespace() => words.extend(word.take()),
            _ => word.get_or_insert_default().push(c),
        }
    }
    if quote.is_some() {
        return Err("a quote is left open".into());
    }
    words.extend(word);
    Ok(words)
}

#[cfg(test)]
mod tests;
