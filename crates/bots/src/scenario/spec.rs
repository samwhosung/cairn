use std::path::Path;

use protocol::flags;
use server::{Config, Rules, View};

use super::file::{At, Bad, Expect, Setting, Text};
use crate::lie::{Clock, Lie, Malformed};
use crate::region::{self, Place, Region};
use crate::track::{Gait, RUN, WALK};

/// What a scenario file asks for, every key read and checked.
#[derive(Clone, Debug)]
pub struct Spec {
    pub name: String,
    pub place: Place,
    pub seconds: f32,
    pub seed: u64,
    /// The server's tunables; its spawns come from the bots.
    pub server: Config,
    pub delay_ms: u32,
    pub jitter_ms: u32,
    pub groups: Vec<Group>,
    pub expects: Vec<Expect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    /// Planned routes about the place: runs, walks, arcs, jumps, standing and looking about.
    Wander,
    /// A straight run along the facing.
    Line,
    Stand,
}

/// Bots alike: how many, what they do, and how they lie.
#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub count: usize,
    pub script: Script,
    pub speed: f32,
    pub gait: Gait,
    pub spawn: Option<[f32; 2]>,
    pub facing_deg: f32,
    pub stop_yd: Option<f32>,
    pub jump_every_s: Option<f32>,
    pub surface: bool,
    /// Frames a second.
    pub hz: f32,
    pub every_frame: bool,
    /// What the bot's clock reads when the server starts.
    pub clock_ms: u32,
    /// On the server's clock from its start.
    pub lie: Option<Lie>,
    /// The liar group this one is the honest twin of.
    pub control_of: Option<usize>,
}

/// The top of a verdict, which an expectation reads when it names no bots.
const VERDICT_FIELDS: [&str; 15] = [
    "scenario",
    "pass",
    "ticks",
    "game_s",
    "players",
    "claims",
    "stale",
    "refused",
    "corrections",
    "decode_errors",
    "groups",
    "hash",
    "expect",
    "speedup",
    "load",
];
const DEFAULT_SECONDS: f32 = 60.0;
const DEFAULT_DELAY_MS: u32 = 20;
const DEFAULT_JITTER_MS: u32 = 10;
const DEFAULT_HZ: f32 = 20.0;
const MOST_HZ: f32 = 1000.0;

pub fn spec(text: Text, file: &Path) -> Result<Spec, Bad> {
    let name = file
        .file_stem()
        .map_or("scenario".into(), |s| s.to_string_lossy().into_owned());
    let mut spec = Spec {
        name,
        place: region::place("flat").ok_or_else(|| bad_place("flat"))?,
        seconds: DEFAULT_SECONDS,
        seed: 1,
        server: Config::default(),
        delay_ms: DEFAULT_DELAY_MS,
        jitter_ms: DEFAULT_JITTER_MS,
        groups: Vec::new(),
        expects: text.expects,
    };
    let mut drafts: Vec<Draft> = Vec::new();
    let mut disk: Vec<&Setting> = Vec::new();
    for s in &text.settings {
        let fault = |what: String| Bad::at(&s.at, what);
        match s.key.as_str() {
            "place.centre" | "place.radius" => disk.push(s),
            key => match key.strip_prefix("bots.") {
                Some(rest) => group_key(&mut drafts, rest, s).map_err(fault)?,
                None => set(&mut spec, s).map_err(fault)?,
            },
        }
    }
    for s in disk {
        let Region::Disk { centre, radius } = &mut spec.place.region else {
            let what = format!("{} is a zone, not a disk to move or size", spec.place.name);
            return Err(Bad::at(&s.at, what));
        };
        let fault = |what: String| Bad::at(&s.at, what);
        if s.key == "place.centre" {
            *centre = xy(&s.value).map_err(fault)?;
        } else {
            *radius = positive(&s.value).map_err(fault)?;
        }
    }
    for d in drafts {
        if !d.counted {
            return Err(Bad::at(
                &d.at,
                format!("bots.{} has no count", d.group.name),
            ));
        }
        let gait_speed = match d.group.gait {
            Gait::Run => RUN,
            Gait::Walk => WALK,
            Gait::Swim => Rules::default().swim,
        };
        spec.groups.push(Group {
            speed: d.speed.unwrap_or(gait_speed),
            ..d.group
        });
    }
    let liars: Vec<usize> = (0..spec.groups.len())
        .filter(|&i| spec.groups[i].lie.is_some())
        .collect();
    for i in liars {
        let name = format!("{}-control", spec.groups[i].name);
        if spec.groups.iter().any(|g| g.name == name) {
            let what = format!("bots.{name} is the name of the liar's honest twin");
            return Err(Bad { at: None, what });
        }
        let twin = Group {
            name,
            lie: None,
            control_of: Some(i),
            ..spec.groups[i].clone()
        };
        spec.groups.push(twin);
    }
    Ok(spec)
}

/// Bots as their keys come in: counted once a count comes, their speed their gait's unless given.
struct Draft {
    group: Group,
    at: At,
    counted: bool,
    speed: Option<f32>,
}

fn bad_place(name: &str) -> Bad {
    Bad {
        at: None,
        what: format!("no place {name}"),
    }
}

fn set(spec: &mut Spec, s: &Setting) -> Result<(), String> {
    let v = s.value.as_str();
    match s.key.as_str() {
        "name" => spec.name = v.to_string(),
        "place" => {
            spec.place =
                region::place(v).ok_or(format!("no place `{v}`: goldshire, elwynn or flat"))?;
        }
        "seconds" => spec.seconds = positive(v)?,
        "seed" => spec.seed = whole(v)?,
        "tick_ms" => spec.server.tick_ms = whole(v)?,
        "client.delay_ms" => spec.delay_ms = whole(v)?,
        "client.jitter_ms" => spec.jitter_ms = whole(v)?,
        key => {
            let knob = if let Some(k) = key.strip_prefix("rules.") {
                rules_knob(&mut spec.server.rules, k)
            } else if let Some(k) = key.strip_prefix("view.") {
                view_knob(&mut spec.server.view, k)
            } else {
                None
            };
            knob.ok_or_else(|| unknown(key))?.set(v)?;
        }
    }
    Ok(())
}

pub enum Knob<'a> {
    F32(&'a mut f32),
    U32(&'a mut u32),
    Usize(&'a mut usize),
    Bool(&'a mut bool),
}

impl Knob<'_> {
    fn set(self, v: &str) -> Result<(), String> {
        match self {
            Self::F32(f) => *f = number(v)?,
            Self::U32(n) => *n = whole(v)?,
            Self::Usize(n) => *n = whole(v)?,
            Self::Bool(b) => *b = yes(v)?,
        }
        Ok(())
    }
}

/// The movement check's tunables, by the name after `rules.`.
pub fn rules_knob<'a>(r: &'a mut Rules, name: &str) -> Option<Knob<'a>> {
    Some(match name {
        "walk" => Knob::F32(&mut r.walk),
        "run" => Knob::F32(&mut r.run),
        "run_back" => Knob::F32(&mut r.run_back),
        "swim" => Knob::F32(&mut r.swim),
        "swim_back" => Knob::F32(&mut r.swim_back),
        "tolerance" => Knob::F32(&mut r.tolerance),
        "slack" => Knob::F32(&mut r.slack),
        "climb" => Knob::F32(&mut r.climb),
        "rise" => Knob::F32(&mut r.rise),
        "fall" => Knob::F32(&mut r.fall),
        "clock_slack_ms" => Knob::U32(&mut r.clock_slack_ms),
        "clock_budget_ms" => Knob::U32(&mut r.clock_budget_ms),
        "bound" => Knob::F32(&mut r.bound),
        "check" => Knob::Bool(&mut r.check),
        _ => return None,
    })
}

/// The view's tunables, by the name after `view.`; its three tiers are `near`, `middle` and
/// `far`, the last reaching as far as the view.
pub fn view_knob<'a>(v: &'a mut View, name: &str) -> Option<Knob<'a>> {
    let [near, middle, far] = &mut v.tiers;
    Some(match name {
        "radius" => Knob::F32(&mut v.radius),
        "grey" => Knob::F32(&mut v.grey),
        "aoi_every" => Knob::U32(&mut v.aoi_every),
        "near.within" => Knob::F32(&mut near.within),
        "near.every" => Knob::U32(&mut near.every),
        "middle.within" => Knob::F32(&mut middle.within),
        "middle.every" => Knob::U32(&mut middle.every),
        "far.every" => Knob::U32(&mut far.every),
        "shed_bytes" => Knob::Usize(&mut v.shed_bytes),
        "shed_ticks" => Knob::U32(&mut v.shed_ticks),
        "kick_bytes" => Knob::Usize(&mut v.kick_bytes),
        "kick_ticks" => Knob::U32(&mut v.kick_ticks),
        _ => return None,
    })
}

fn unknown(key: &str) -> String {
    format!("`{key}` is not a key a scenario sets")
}

fn group_key(drafts: &mut Vec<Draft>, rest: &str, s: &Setting) -> Result<(), String> {
    let (name, field) = rest.split_once('.').ok_or_else(|| unknown(&s.key))?;
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "`{name}` is not a name for bots: lower case, digits and -"
        ));
    }
    if VERDICT_FIELDS.contains(&name) {
        return Err(format!("`{name}` names a field of the verdict, not bots"));
    }
    let i = if let Some(i) = drafts.iter().position(|d| d.group.name == name) {
        i
    } else {
        drafts.push(Draft {
            group: group(name),
            at: s.at.clone(),
            counted: false,
            speed: None,
        });
        drafts.len() - 1
    };
    let d = &mut drafts[i];
    let g = &mut d.group;
    let v = s.value.as_str();
    match field {
        "count" => {
            g.count = whole(v)?;
            d.counted = true;
        }
        "script" => {
            g.script = match v {
                "wander" => Script::Wander,
                "line" => Script::Line,
                "stand" => Script::Stand,
                _ => return Err(format!("no script `{v}`: wander, line or stand")),
            };
        }
        "gait" => {
            g.gait = match v {
                "run" => Gait::Run,
                "walk" => Gait::Walk,
                "swim" => Gait::Swim,
                _ => return Err(format!("no gait `{v}`: run, walk or swim")),
            };
        }
        "speed" => d.speed = Some(positive(v)?),
        "spawn" => g.spawn = Some(xy(v)?),
        "facing" => g.facing_deg = number(v)?,
        "stop" => g.stop_yd = Some(positive(v)?),
        "jump_every_s" => g.jump_every_s = Some(positive(v)?),
        "surface" => g.surface = yes(v)?,
        "hz" => {
            g.hz = positive(v)?;
            if g.hz > MOST_HZ {
                return Err(format!("{v} frames a second is past {MOST_HZ}"));
            }
        }
        "claims" => {
            g.every_frame = match v {
                "send-law" => false,
                "every-frame" => true,
                _ => return Err(format!("no claims `{v}`: send-law or every-frame")),
            };
        }
        "clock_ms" => g.clock_ms = whole(v)?,
        field => {
            let known = match field.strip_prefix("lie.") {
                Some(key) => lie_key(g.lie.get_or_insert_with(Lie::default), key, v)?,
                None => false,
            };
            if !known {
                return Err(unknown(&s.key));
            }
        }
    }
    Ok(())
}

fn group(name: &str) -> Group {
    Group {
        name: name.to_string(),
        count: 0,
        script: Script::Wander,
        speed: RUN,
        gait: Gait::Run,
        spawn: None,
        facing_deg: 0.0,
        stop_yd: None,
        jump_every_s: None,
        surface: false,
        hz: DEFAULT_HZ,
        every_frame: false,
        clock_ms: 0,
        lie: None,
        control_of: None,
    }
}

/// Sets a lie's key; false when there is no such key.
fn lie_key(l: &mut Lie, key: &str, v: &str) -> Result<bool, String> {
    match key {
        "from_s" => l.from_ms = ms(v)?,
        "to_s" => l.to_ms = ms(v)?,
        "factor" => l.factor = Some(positive(v)?),
        "shift" => l.shift = xyz(v)?,
        "once" => l.once = yes(v)?,
        "rise" => l.rise = number(v)?,
        "hover" => l.hover = yes(v)?,
        "through" => l.through = yes(v)?,
        "set" => l.set_flags = flag_names(v)?,
        "clear" => l.clear_flags = flag_names(v)?,
        "clock_rate" => l.clock = Clock::Rate(positive(v)?),
        "clock_back_ms" => l.clock = Clock::Back(whole(v)?),
        "clock_stall" => {
            l.clock = if yes(v)? { Clock::Stall } else { Clock::Body };
        }
        "launch" => l.launch = positive(v)?,
        "malformed" => {
            l.malformed = Some(match v {
                "nan" => Malformed::NotANumber,
                "bound" => Malformed::PastTheBound,
                _ => return Err(format!("no malformed `{v}`: nan or bound")),
            });
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn flag_names(v: &str) -> Result<u32, String> {
    v.split_whitespace().try_fold(0, |all, name| {
        let bit = match name {
            "forward" => flags::FORWARD,
            "backward" => flags::BACKWARD,
            "strafe_left" => flags::STRAFE_LEFT,
            "strafe_right" => flags::STRAFE_RIGHT,
            "moving" => flags::ANY_MOVE,
            "walk" => flags::WALK_MODE,
            "swim" => flags::SWIMMING,
            "fall" => flags::FALLING,
            _ => {
                return Err(format!(
                    "no flag `{name}`: forward, backward, strafe_left, strafe_right, moving, walk, swim or fall"
                ));
            }
        };
        Ok(all | bit)
    })
}

fn number(v: &str) -> Result<f32, String> {
    v.parse::<f32>()
        .ok()
        .filter(|f| f.is_finite())
        .ok_or(format!("`{v}` is not a number"))
}

fn positive(v: &str) -> Result<f32, String> {
    number(v)
        .ok()
        .filter(|&f| f > 0.0)
        .ok_or(format!("`{v}` is not a number above zero"))
}

fn whole<T: std::str::FromStr>(v: &str) -> Result<T, String> {
    v.parse()
        .map_err(|_| format!("`{v}` is not a whole number"))
}

fn ms(v: &str) -> Result<u32, String> {
    let secs = number(v)?;
    if secs < 0.0 {
        return Err(format!("`{v}` is before the start"));
    }
    Ok((secs * 1000.0).round() as u32)
}

fn yes(v: &str) -> Result<bool, String> {
    match v {
        "yes" | "true" => Ok(true),
        "no" | "false" => Ok(false),
        _ => Err(format!("`{v}` is not yes or no")),
    }
}

fn numbers<const N: usize>(v: &str) -> Result<[f32; N], String> {
    let parsed: Vec<f32> = v.split_whitespace().map(number).collect::<Result<_, _>>()?;
    parsed
        .try_into()
        .map_err(|_| format!("`{v}` is not {N} numbers"))
}

fn xy(v: &str) -> Result<[f32; 2], String> {
    numbers(v)
}

fn xyz(v: &str) -> Result<[f32; 3], String> {
    numbers(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names of a struct's own fields, as its `Debug` shows them.
    fn fields(debug: &str) -> Vec<String> {
        let body =
            &debug[debug.find('{').expect("a struct") + 1..debug.rfind('}').expect("closed")];
        let mut depth = 0;
        let mut names = Vec::new();
        let mut word = String::new();
        for c in body.chars() {
            match c {
                '{' | '[' => depth += 1,
                '}' | ']' => depth -= 1,
                ':' if depth == 0 && !word.trim().is_empty() => {
                    names.push(word.trim().to_string());
                    word.clear();
                }
                ',' if depth == 0 => word.clear(),
                _ if depth == 0 => word.push(c),
                _ => {}
            }
        }
        names
    }

    #[test]
    fn every_tunable_of_the_movement_check_and_the_view_is_a_key() {
        let mut rules = Rules::default();
        let names = fields(&format!("{rules:?}"));
        assert!(names.len() >= 14, "{names:?}");
        for name in names {
            assert!(rules_knob(&mut rules, &name).is_some(), "rules.{name}");
        }
        let mut view = View::default();
        for name in fields(&format!("{view:?}")) {
            let knobs: Vec<String> = if name == "tiers" {
                [
                    "near.within",
                    "near.every",
                    "middle.within",
                    "middle.every",
                    "far.every",
                ]
                .map(String::from)
                .to_vec()
            } else {
                vec![name]
            };
            for k in knobs {
                assert!(view_knob(&mut view, &k).is_some(), "view.{k}");
            }
        }
    }

    fn settings(lines: &[(&str, &str)]) -> Text {
        let at = |line| At {
            file: "t.scenario".into(),
            line,
        };
        Text {
            settings: lines
                .iter()
                .enumerate()
                .map(|(i, (k, v))| Setting {
                    key: (*k).into(),
                    value: (*v).into(),
                    at: at(i + 1),
                })
                .collect(),
            expects: Vec::new(),
        }
    }

    #[test]
    fn keys_set_the_knobs_and_a_liar_gets_an_honest_twin() {
        let text = settings(&[
            ("rules.run", "14"),
            ("view.near.every", "2"),
            ("place", "flat"),
            ("place.radius", "80"),
            ("bots.crowd.count", "3"),
            ("bots.fast.count", "1"),
            ("bots.fast.lie.factor", "3"),
            ("bots.fast.gait", "walk"),
            ("bots.fast.lie.set", "walk fall"),
        ]);
        let s = spec(text, Path::new("x/fast.scenario")).expect("a spec");
        assert_eq!(s.name, "fast");
        assert!((s.server.rules.run - 14.0).abs() < 1e-6);
        assert_eq!(s.server.view.tiers[0].every, 2);
        assert!(
            matches!(s.place.region, Region::Disk { radius, .. } if (radius - 80.0).abs() < 1e-6)
        );
        let names: Vec<&str> = s.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["crowd", "fast", "fast-control"]);
        let (fast, twin) = (&s.groups[1], &s.groups[2]);
        assert!(fast.gait == Gait::Walk && (fast.speed - WALK).abs() < 1e-6);
        assert_eq!(
            fast.lie.map(|l| (l.factor.map(|f| f as u32), l.set_flags)),
            Some((Some(3), flags::WALK_MODE | flags::FALLING))
        );
        assert_eq!((twin.lie, twin.control_of, twin.count), (None, Some(1), 1));
    }

    #[test]
    fn an_unknown_key_is_a_fault_at_its_line() {
        let fault = |lines: &[(&str, &str)]| {
            spec(settings(lines), Path::new("t.scenario"))
                .expect_err("a fault")
                .to_string()
        };
        assert_eq!(
            fault(&[("seconds", "9"), ("rules.speed", "7")]),
            "t.scenario:2: `rules.speed` is not a key a scenario sets"
        );
        assert_eq!(
            fault(&[("bots.a.count", "1"), ("bots.a.lie.fast", "2")]),
            "t.scenario:2: `bots.a.lie.fast` is not a key a scenario sets"
        );
        assert_eq!(
            fault(&[("bots.a.speed", "2")]),
            "t.scenario:1: bots.a has no count"
        );
        assert_eq!(
            fault(&[("bots.a.count", "1"), ("bots.a.hz", "2000")]),
            "t.scenario:2: 2000 frames a second is past 1000"
        );
    }
}
