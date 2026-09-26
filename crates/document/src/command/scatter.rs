use super::{AREA, Words};
use crate::grammar::{Args, area};
use crate::mask::{self, Mask, Span};
use crate::shape::Shape;
use crate::text::number;

#[derive(Clone, Debug, PartialEq)]
pub struct Scatter {
    pub models: Vec<String>,
    pub area: Shape,
    pub count: usize,
    /// No two closer than this, in yards; two models keep the mean of theirs.
    pub apart: PerModel<f64>,
    pub scale: PerModel<(f64, f64)>,
    /// A fixed facing, or `None` for any.
    pub facing: Option<f64>,
    pub seed: u64,
    pub slope: PerModel<Span>,
    /// Tilted as the ground is, rather than upright.
    pub lean: PerModel<bool>,
    pub mask: Mask,
}

/// A rule a scatter gives its models.
#[derive(Clone, Debug, PartialEq)]
pub enum PerModel<T> {
    /// Each keeps its own, from where the install places it, or, for a model the install never
    /// places on its ground, any slope, scale 1 and upright, and no spacing.
    Own,
    All(T),
    /// One for each model, in the order the scatter names them.
    Each(Vec<T>),
}

impl<T: Clone + PartialEq> PerModel<T> {
    pub fn given(&self, model: usize) -> Option<&T> {
        match self {
            PerModel::Own => None,
            PerModel::All(v) => Some(v),
            PerModel::Each(v) => v.get(model),
        }
    }

    fn texts(&self, text: impl Fn(&T) -> String) -> Vec<String> {
        match self {
            PerModel::Own => Vec::new(),
            PerModel::All(v) => vec![text(v)],
            PerModel::Each(v) if v.iter().all(|x| *x == v[0]) => vec![text(&v[0])],
            PerModel::Each(v) => v.iter().map(text).collect(),
        }
    }
}

pub const FLAGS: [&str; 8] = [
    "models", "count", "apart", "scale", "facing", "seed", "slope", "stands",
];

const UPRIGHT: &str = "upright";
const LEANING: &str = "leaning";

pub fn stands(s: &str) -> Result<bool, String> {
    match s {
        UPRIGHT => Ok(false),
        LEANING => Ok(true),
        _ => Err(format!(
            "--stands {s:?}: a model stands {UPRIGHT} or {LEANING}"
        )),
    }
}

pub fn stands_text(lean: bool) -> String {
    if lean { LEANING } else { UPRIGHT }.to_owned()
}

pub fn allowed() -> Vec<&'static str> {
    [&AREA[..], &FLAGS[..], &mask::FLAGS[..]].concat()
}

fn scale_range(s: &str) -> Result<(f64, f64), String> {
    Ok(match s.split_once("..") {
        Some((lo, hi)) => (number(lo, "--scale")?, number(hi, "--scale")?),
        None => (number(s, "--scale")?, number(s, "--scale")?),
    })
}

pub fn apart(s: &str) -> Result<f64, String> {
    let d: f64 = number(s, "--apart")?;
    if d >= fits::rules::LEAST_APART && d.is_finite() {
        Ok(d)
    } else {
        Err(format!(
            "--apart must be at least {} yd",
            fits::rules::LEAST_APART
        ))
    }
}

fn per_model<T>(
    words: Option<&[String]>,
    read: impl Fn(&str) -> Result<T, String>,
) -> Result<PerModel<T>, String> {
    Ok(match words {
        None => PerModel::Own,
        Some([one]) => PerModel::All(read(one)?),
        Some(each) => PerModel::Each(each.iter().map(|w| read(w)).collect::<Result<_, _>>()?),
    })
}

pub fn parse(a: &Args) -> Result<Scatter, String> {
    let models: Vec<String> = a
        .words("models")
        .ok_or("--models A B ...: the models to spread")?
        .iter()
        .flat_map(|s| s.split(',').map(str::to_owned))
        .filter(|s| !s.is_empty())
        .collect();
    if models.is_empty() {
        return Err("scatter needs at least one model".into());
    }
    let n = models.len();
    let per = |flag: &str| -> Result<Option<&[String]>, String> {
        match a.words(flag) {
            Some(w) if w.len() != 1 && w.len() != n => Err(format!(
                "--{flag}: one value for every model or one for each of the {n}, not {}",
                w.len()
            )),
            w => Ok(w),
        }
    };
    Ok(Scatter {
        area: area(a)?,
        count: a.num("count")?.ok_or("--count N: how many")? as usize,
        apart: per_model(per("apart")?, apart)?,
        scale: per_model(per("scale")?, scale_range)?,
        facing: a.num("facing")?,
        seed: a.num("seed")?.unwrap_or(0.0) as u64,
        slope: per_model(per("slope")?, |s| Span::parse(s, "--slope"))?,
        lean: per_model(per("stands")?, stands)?,
        mask: Mask::parse(a)?,
        models,
    })
}

pub fn words(w: &mut Words, s: &Scatter) {
    w.word("scatter");
    w.flag("models", s.models.iter().cloned());
    w.shape(&s.area);
    w.flag("count", [s.count.to_string()]);
    let mut rule = |flag: &str, texts: Vec<String>| {
        if !texts.is_empty() {
            w.flag(flag, texts);
        }
    };
    rule("apart", s.apart.texts(ToString::to_string));
    rule(
        "scale",
        s.scale.texts(|&(lo, hi)| {
            if f64::to_bits(lo) == f64::to_bits(hi) {
                lo.to_string()
            } else {
                format!("{lo}..{hi}")
            }
        }),
    );
    if let Some(f) = s.facing {
        w.num_flag("facing", f);
    }
    w.flag("seed", [s.seed.to_string()]);
    let mut rule = |flag: &str, texts: Vec<String>| {
        if !texts.is_empty() {
            w.flag(flag, texts);
        }
    };
    rule("slope", s.slope.texts(Span::text));
    rule("stands", s.lean.texts(|&l| stands_text(l)));
    s.mask.words(w);
}
