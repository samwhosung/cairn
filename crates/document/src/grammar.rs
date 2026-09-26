use std::collections::BTreeMap;

use crate::shape::Shape;
use crate::text::number;

#[derive(Debug, Default)]
pub struct Args {
    pub pos: Vec<String>,
    pub flags: BTreeMap<String, Vec<String>>,
    twice: Option<String>,
}

impl Args {
    pub fn parse(words: &[String]) -> Args {
        let mut a = Args::default();
        let mut current: Option<String> = None;
        for w in words {
            if let Some(f) = w.strip_prefix("--") {
                if a.flags.insert(f.to_owned(), Vec::new()).is_some() {
                    a.twice.get_or_insert_with(|| f.to_owned());
                }
                current = Some(f.to_owned());
            } else if let Some(values) = current.as_ref().and_then(|c| a.flags.get_mut(c)) {
                values.push(w.clone());
            } else {
                a.pos.push(w.clone());
            }
        }
        a
    }

    pub fn has(&self, f: &str) -> bool {
        self.flags.contains_key(f)
    }

    pub fn words(&self, f: &str) -> Option<&[String]> {
        self.flags.get(f).map(Vec::as_slice)
    }

    pub fn one(&self, f: &str) -> Result<Option<&str>, String> {
        match self.flags.get(f).map(Vec::as_slice) {
            None => Ok(None),
            Some([v]) => Ok(Some(v)),
            Some(v) => Err(format!("--{f} takes one value, got {}", v.len())),
        }
    }

    pub fn num(&self, f: &str) -> Result<Option<f64>, String> {
        self.one(f)?
            .map(|s| number(s, &format!("--{f}")))
            .transpose()
    }

    /// An error naming every flag this command doesn't take, or one given twice.
    pub fn allow_only(&self, known: &[&str]) -> Result<(), String> {
        if let Some(f) = &self.twice {
            return Err(format!("--{f} is given twice"));
        }
        let unknown: Vec<String> = self
            .flags
            .keys()
            .filter(|k| !known.contains(&k.as_str()))
            .map(|k| format!("--{k}"))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let takes: Vec<String> = known.iter().map(|k| format!("--{k}")).collect();
        Err(format!(
            "unknown flag{} {}; this command takes {}",
            if unknown.len() > 1 { "s" } else { "" },
            unknown.join(" "),
            if takes.is_empty() {
                "none".to_owned()
            } else {
                takes.join(" ")
            }
        ))
    }

    pub fn first(&self, want: &str) -> Result<&str, String> {
        self.pos
            .first()
            .map(String::as_str)
            .ok_or_else(|| want.to_owned())
    }
}

pub fn point(s: &str) -> Result<[f64; 2], String> {
    match s.split(',').collect::<Vec<_>>()[..] {
        [x, y] => Ok([number(x, s)?, number(y, s)?]),
        _ => Err(format!("{s:?}: want X,Y (yards east, south)")),
    }
}

pub fn point_z(s: &str) -> Result<([f64; 2], Option<f64>), String> {
    match s.split(',').collect::<Vec<_>>()[..] {
        [x, y] => Ok(([number(x, s)?, number(y, s)?], None)),
        [x, y, z] => Ok(([number(x, s)?, number(y, s)?], Some(number(z, s)?))),
        _ => Err(format!("{s:?}: want X,Y or X,Y,Z")),
    }
}

pub fn shape(w: &[String]) -> Result<Shape, String> {
    let kind = w.first().ok_or("no outline")?;
    let finite = |v: f64, what: &str| {
        if v.is_finite() && v >= 0.0 {
            Ok(v)
        } else {
            Err(format!("{what} {v} is not a size"))
        }
    };
    Ok(match kind.as_str() {
        "circle" => match w {
            [_, c, r] => Shape::Circle {
                c: point(c)?,
                r: finite(number(r, "a radius")?, "the radius")?,
            },
            _ => return Err("circle X,Y R".into()),
        },
        "line" => {
            let k = w
                .iter()
                .position(|s| s == "width")
                .ok_or("line X,Y ... width W")?;
            let mut pts = Vec::new();
            let mut z = Vec::new();
            for s in &w[1..k] {
                let (p, h) = point_z(s)?;
                pts.push(p);
                z.push(h);
            }
            if pts.is_empty() {
                return Err("a line needs a point".into());
            }
            let width = w.get(k + 1).ok_or("width W")?;
            Shape::Line {
                pts,
                z,
                width: finite(number(width, "a width")?, "the width")?,
            }
        }
        "rect" => match w {
            [_, a, b] => Shape::Rect {
                a: point(a)?,
                b: point(b)?,
            },
            _ => return Err("rect X,Y X,Y".into()),
        },
        "poly" => {
            let pts = w[1..]
                .iter()
                .map(|s| point(s))
                .collect::<Result<Vec<_>, _>>()?;
            if pts.len() < 3 {
                return Err("a polygon needs three points".into());
            }
            Shape::Poly { pts }
        }
        k => {
            return Err(format!(
                "{k:?} is not an outline (circle, line, rect, poly)"
            ));
        }
    })
}

pub fn brush(a: &Args) -> Result<Shape, String> {
    if let Some(at) = a.one("at")? {
        let r = a.num("radius")?.ok_or("--at needs --radius R (yards)")?;
        if !(r > 0.0 && r.is_finite()) {
            return Err("--radius must be positive".into());
        }
        return Ok(Shape::Circle { c: point(at)?, r });
    }
    if let Some(pts) = a.words("line") {
        let w = a.num("width")?.ok_or("--line needs --width W (yards)")?;
        if !(w > 0.0 && w.is_finite()) {
            return Err("--width must be positive".into());
        }
        let mut words = vec!["line".to_owned()];
        words.extend(pts.iter().cloned());
        words.push("width".into());
        words.push(w.to_string());
        return shape(&words);
    }
    Err("say where: --at X,Y --radius R, or --line X,Y X,Y ... --width W".into())
}

pub fn area(a: &Args) -> Result<Shape, String> {
    for kind in ["rect", "poly"] {
        if let Some(v) = a.words(kind) {
            let mut w = vec![kind.to_owned()];
            w.extend(v.iter().cloned());
            return shape(&w);
        }
    }
    if a.has("at") || a.has("line") {
        return brush(a);
    }
    Err(
        "say where: --at X,Y --radius R, --rect X,Y X,Y, --poly X,Y ..., or --line X,Y ... \
         --width W"
            .into(),
    )
}

pub fn check_word(w: &str) -> Result<(), String> {
    if w.chars().any(char::is_control) || (w.contains('"') && w.contains('\'')) {
        return Err(format!(
            "{w:?}: a word can hold no line break or tab, nor both kinds of quote"
        ));
    }
    Ok(())
}

fn quote(w: &str) -> String {
    let bare = !w.is_empty()
        && !w
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"');
    if bare {
        w.to_owned()
    } else if w.contains('"') {
        format!("'{w}'")
    } else {
        format!("\"{w}\"")
    }
}

/// A line's words: quotes group, and a backslash is only a backslash.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut have = false;
    let mut open: Option<char> = None;
    for c in line.chars() {
        match (open, c) {
            (None, c) if c.is_whitespace() => {
                if have {
                    out.push(std::mem::take(&mut cur));
                    have = false;
                }
            }
            (None, '\'' | '"') => {
                open = Some(c);
                have = true;
            }
            (Some(q), c) if c == q => open = None,
            (_, c) => {
                cur.push(c);
                have = true;
            }
        }
    }
    if open.is_some() {
        return Err(format!("an unclosed quote in {line:?}"));
    }
    if have {
        out.push(cur);
    }
    Ok(out)
}

/// Words that pass [`check_word`] joined into a line that splits back into them.
pub fn join(words: &[String]) -> String {
    words.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &[&str]) -> Vec<String> {
        s.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn flags_take_negative_numbers() {
        let a = Args::parse(&words(&[
            "z", "-3", "--at", "1,2", "--radius", "5", "--line", "0,0", "5,-1,3",
        ]));
        assert_eq!(a.pos, ["z", "-3"]);
        assert_eq!(a.words("line").map(<[String]>::len), Some(2));
        assert!(matches!(brush(&a), Ok(Shape::Circle { r, .. }) if (r - 5.0).abs() < 1e-12));
    }

    #[test]
    fn lines_split_back_into_their_words() {
        let w = words(&[
            "paint",
            "Tileset\\Swamp of Sorrows\\A.blp",
            "it's",
            "a\"b",
            "Two  Spaces",
            "",
            "--at",
            "1,2",
        ]);
        assert!(w.iter().all(|w| check_word(w).is_ok()));
        assert_eq!(split(&join(&w)), Ok(w));
        assert!(split("a \"b").is_err());
        assert!(check_word("a'b\"c").is_err() && check_word("a\nb").is_err());
    }
}
