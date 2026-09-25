use std::cmp::Ordering;
use std::fmt;
use std::path::{Path, PathBuf};

/// A line of a scenario file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct At {
    pub file: PathBuf,
    pub line: usize,
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

/// Why a scenario cannot run as written, and where its files say so.
#[derive(Debug)]
pub struct Bad {
    pub at: Option<At>,
    pub what: String,
}

impl Bad {
    pub fn at(at: &At, what: impl Into<String>) -> Self {
        Self {
            at: Some(at.clone()),
            what: what.into(),
        }
    }
}

impl fmt::Display for Bad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.at {
            Some(at) => write!(f, "{at}: {}", self.what),
            None => write!(f, "{}", self.what),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Setting {
    pub key: String,
    pub value: String,
    pub at: At,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "=" | "==" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            _ => return None,
        })
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }

    /// Whether `got` stands in this relation to `want`; a missing number only equals another.
    pub fn holds(self, got: Option<f64>, want: Option<f64>) -> bool {
        match (got, want) {
            (Some(g), Some(w)) => {
                let order = g.partial_cmp(&w);
                match self {
                    Self::Eq => order == Some(Ordering::Equal),
                    Self::Ne => order != Some(Ordering::Equal),
                    Self::Lt => order == Some(Ordering::Less),
                    Self::Le => matches!(order, Some(Ordering::Less | Ordering::Equal)),
                    Self::Gt => order == Some(Ordering::Greater),
                    Self::Ge => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
                }
            }
            (None, None) => matches!(self, Self::Eq | Self::Le | Self::Ge),
            _ => self == Self::Ne,
        }
    }
}

/// `expect PATH OP VALUE`: a number of the verdict, a relation, and a number or `none`.
#[derive(Clone, Debug)]
pub struct Expect {
    pub path: String,
    pub op: Op,
    pub want: Option<f64>,
    pub at: At,
}

impl fmt::Display for Expect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let want = self.want.map_or("none".to_string(), |w| w.to_string());
        write!(f, "{} {} {want}", self.path, self.op.symbol())
    }
}

/// A scenario as its files say it: every setting, an overlay's in place of its base's, in the
/// order their keys first appear; and every expectation, an overlay's in place of its base's on
/// the same number.
#[derive(Debug, Default)]
pub struct Text {
    pub settings: Vec<Setting>,
    pub expects: Vec<Expect>,
}

pub fn read(path: &Path) -> Result<Text, Bad> {
    read_with(path, &|p| std::fs::read_to_string(p), &mut Vec::new())
}

type Load<'a> = dyn Fn(&Path) -> std::io::Result<String> + 'a;

fn read_with(path: &Path, load: &Load<'_>, reading: &mut Vec<PathBuf>) -> Result<Text, Bad> {
    let text = load(path).map_err(|e| Bad {
        at: None,
        what: format!("{}: {e}", path.display()),
    })?;
    reading.push(path.to_path_buf());
    let mut own = Text::default();
    let mut base: Option<(PathBuf, At)> = None;
    for (i, raw) in text.lines().enumerate() {
        let at = At {
            file: path.to_path_buf(),
            line: i + 1,
        };
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("expect ") {
            own.expects.push(expectation(rest, at)?);
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(Bad::at(&at, "want `key = value` or `expect PATH OP VALUE`"));
        };
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty() || !key.chars().all(is_key_char) {
            return Err(Bad::at(&at, format!("`{key}` is not a key")));
        }
        if key == "base" {
            if base.is_some() {
                return Err(Bad::at(&at, "a second base"));
            }
            let dir = path.parent().unwrap_or(Path::new("."));
            base = Some((dir.join(value), at));
            continue;
        }
        if let Some(first) = own.settings.iter().find(|s| s.key == key) {
            return Err(Bad::at(
                &at,
                format!("`{key}` again: it was set at line {}", first.at.line),
            ));
        }
        own.settings.push(Setting {
            key: key.to_string(),
            value: value.to_string(),
            at,
        });
    }
    let mut merged = match base {
        Some((file, at)) if reading.contains(&file) => {
            return Err(Bad::at(&at, format!("{} is its own base", file.display())));
        }
        Some((file, at)) => read_with(&file, load, reading).map_err(|e| Bad {
            at: e.at.or(Some(at)),
            what: e.what,
        })?,
        None => Text::default(),
    };
    for s in own.settings {
        match merged.settings.iter_mut().find(|m| m.key == s.key) {
            Some(m) => *m = s,
            None => merged.settings.push(s),
        }
    }
    merged
        .expects
        .retain(|e| !own.expects.iter().any(|o| o.path == e.path));
    merged.expects.extend(own.expects);
    reading.pop();
    Ok(merged)
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
}

fn expectation(rest: &str, at: At) -> Result<Expect, Bad> {
    let words: Vec<&str> = rest.split_whitespace().collect();
    let [path, op, want] = words[..] else {
        return Err(Bad::at(&at, "want `expect PATH OP VALUE`"));
    };
    let op = Op::parse(op)
        .ok_or_else(|| Bad::at(&at, format!("`{op}` is not one of == != < <= > >=")))?;
    let want = match want {
        "none" => None,
        w => Some(
            w.parse::<f64>()
                .map_err(|_| Bad::at(&at, format!("`{w}` is not a number or `none`")))?,
        ),
    };
    Ok(Expect {
        path: path.to_string(),
        op,
        want,
        at,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn read_all(files: &[(&str, &str)], path: &str) -> Result<Text, Bad> {
        let files: HashMap<PathBuf, String> = files
            .iter()
            .map(|(p, t)| (PathBuf::from(p), (*t).to_string()))
            .collect();
        let load = |p: &Path| {
            files
                .get(p)
                .cloned()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        };
        read_with(Path::new(path), &load, &mut Vec::new())
    }

    #[test]
    fn an_overlay_replaces_its_bases_settings_and_expectations_and_keeps_their_order() {
        let base = "# the base\nseconds = 60\nrules.run = 7\nexpect a.corrections > 0\nexpect claims > 1\n";
        let variant =
            "base = base.scenario\nrules.run = 14 # faster\nname = v\nexpect a.corrections == 0\n";
        let files = [("s/base.scenario", base), ("s/variant.scenario", variant)];
        let text = read_all(&files, "s/variant.scenario").expect("a scenario");
        let settings: Vec<(&str, &str, usize)> = text
            .settings
            .iter()
            .map(|s| (s.key.as_str(), s.value.as_str(), s.at.line))
            .collect();
        assert_eq!(
            settings,
            [
                ("seconds", "60", 2),
                ("rules.run", "14", 2),
                ("name", "v", 3)
            ]
        );
        assert_eq!(text.settings[1].at.file, Path::new("s/variant.scenario"));
        let expects: Vec<String> = text.expects.iter().map(ToString::to_string).collect();
        assert_eq!(expects, ["claims > 1", "a.corrections == 0"]);
    }

    #[test]
    fn a_fault_names_its_file_and_line() {
        let files = [
            ("bad.scenario", "seconds = 60\n\nexpect claims ~ 3\n"),
            ("twice.scenario", "seed = 1\nseed = 2\n"),
            ("orphan.scenario", "base = missing.scenario\n"),
            ("own.scenario", "base = own.scenario\n"),
            ("key.scenario", "seconds = 6\nRules.Run = 7\n"),
        ];
        let fault = |path: &str| read_all(&files, path).expect_err(path).to_string();
        assert_eq!(
            fault("bad.scenario"),
            "bad.scenario:3: `~` is not one of == != < <= > >="
        );
        assert!(fault("twice.scenario").starts_with("twice.scenario:2: `seed` again"));
        assert!(fault("orphan.scenario").starts_with("orphan.scenario:1: missing.scenario:"));
        assert!(fault("own.scenario").contains("its own base"));
        assert!(fault("key.scenario").starts_with("key.scenario:2: `Rules.Run` is not a key"));
    }

    #[test]
    fn none_is_only_equal_to_none() {
        assert!(Op::Eq.holds(None, None));
        assert!(!Op::Eq.holds(Some(0.0), None));
        assert!(Op::Ne.holds(Some(0.0), None));
        assert!(!Op::Lt.holds(None, Some(1.0)));
        assert!(Op::Le.holds(Some(1.0), Some(1.0)));
    }
}
