use std::fmt::{self, Write};

use server::{Summary, Why};

use super::file::{Bad, Expect};
use super::run::{Account, Outcome, liar_of};
use super::spec::Spec;

/// A value of the verdict, as JSON writes it.
#[derive(Clone, Debug)]
pub enum Value {
    Count(u64),
    Number(f64),
    Text(String),
    Yes(bool),
    Nothing,
    Object(Vec<(String, Value)>),
    List(Vec<Value>),
}

impl Value {
    fn object(fields: Vec<(&str, Value)>) -> Self {
        Self::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The number at `path`, a key for each object it goes into, or the null that stands for a
    /// number there is none of, such as the time a liar never caught was caught at.
    fn number_at(&self, path: &[&str]) -> Option<&Self> {
        let mut v = self;
        for key in path {
            v = v.get(key)?;
        }
        matches!(v, Self::Count(_) | Self::Number(_) | Self::Nothing).then_some(v)
    }

    fn number(&self) -> Option<f64> {
        match *self {
            Self::Count(n) => Some(n as f64),
            Self::Number(n) => Some(n),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(n) => write!(f, "{n}"),
            Self::Number(n) => write!(f, "{n:.3}"),
            Self::Text(s) => write!(f, "\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            Self::Yes(b) => write!(f, "{b}"),
            Self::Nothing => f.write_str("null"),
            Self::Object(fields) => {
                f.write_char('{')?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    let sep = if i == 0 { "" } else { "," };
                    write!(f, "{sep}\"{k}\":{v}")?;
                }
                f.write_char('}')
            }
            Self::List(items) => {
                f.write_char('[')?;
                for (i, v) in items.iter().enumerate() {
                    let sep = if i == 0 { "" } else { "," };
                    write!(f, "{sep}{v}")?;
                }
                f.write_char(']')
            }
        }
    }
}

fn refused(counts: &[u64; Why::ALL.len()]) -> Value {
    let mut fields = vec![("all".to_string(), Value::Count(counts.iter().sum()))];
    for (why, &n) in Why::ALL.iter().zip(counts) {
        fields.push((format!("{why:?}").to_lowercase(), Value::Count(n)));
    }
    Value::Object(fields)
}

fn sum(accounts: &[&Account], f: impl Fn(&Account) -> u64) -> Value {
    Value::Count(accounts.iter().map(|a| f(a)).sum())
}

/// Everything the scenario measured that the same file measures the same on any machine.
pub fn measured(spec: &Spec, o: &Outcome) -> Value {
    let liar_of = liar_of(spec, &o.groups);
    let members =
        |g: usize| -> Vec<usize> { (0..o.groups.len()).filter(|&b| o.groups[b] == g).collect() };
    let mut groups = Vec::new();
    for (gi, g) in spec.groups.iter().enumerate() {
        if g.control_of.is_some() {
            continue;
        }
        let bots = members(gi);
        let accounts: Vec<&Account> = bots.iter().map(|&b| &o.accounts[b]).collect();
        let mut refusals = [0; Why::ALL.len()];
        for a in &accounts {
            for (all, n) in refusals.iter_mut().zip(a.refused) {
                *all += n;
            }
        }
        let corrections: u64 = bots.iter().map(|&b| o.tallies[b].corrections).sum();
        let mut fields = vec![
            ("bots", Value::Count(bots.len() as u64)),
            ("claims", sum(&accounts, |a| a.claims)),
            ("accepted", sum(&accounts, |a| a.accepted)),
            ("stale", sum(&accounts, |a| a.stale)),
            ("refused", refused(&refusals)),
            ("corrections", Value::Count(corrections)),
        ];
        if g.lie.is_some() {
            fields.extend(lies(o, &bots, &accounts, &liar_of, gi, spec));
        }
        groups.push((g.name.clone(), Value::object(fields)));
    }
    let all: Vec<&Account> = o.accounts.iter().collect();
    let mut refusals = [0; Why::ALL.len()];
    for a in &all {
        for (t, n) in refusals.iter_mut().zip(a.refused) {
            *t += n;
        }
    }
    let game_s = o.ticks.len() as f64 * f64::from(o.tick_ms) / 1000.0;
    Value::object(vec![
        ("scenario", Value::Text(spec.name.clone())),
        ("ticks", Value::Count(o.ticks.len() as u64)),
        ("game_s", Value::Number(game_s)),
        (
            "players",
            Value::Count(
                o.ticks
                    .iter()
                    .map(|t| u64::from(t.players))
                    .max()
                    .unwrap_or(0),
            ),
        ),
        ("claims", sum(&all, |a| a.claims)),
        ("stale", sum(&all, |a| a.stale)),
        ("refused", refused(&refusals)),
        (
            "corrections",
            Value::Count(o.tallies.iter().map(|t| t.corrections).sum()),
        ),
        (
            "decode_errors",
            Value::Count(o.tallies.iter().map(|t| t.decode_errors).sum()),
        ),
        ("groups", Value::Object(groups)),
        (
            "hash",
            Value::Text(format!("{:016x}", o.ticks.last().map_or(0, |t| t.hash))),
        ),
    ])
}

fn lies(
    o: &Outcome,
    bots: &[usize],
    accounts: &[&Account],
    liar_of: &[Option<usize>],
    gi: usize,
    spec: &Spec,
) -> Vec<(&'static str, Value)> {
    let tallies: Vec<_> = bots.iter().map(|&b| &o.tallies[b]).collect();
    let after = |at: fn(&Account) -> Option<u32>| {
        let ms = bots
            .iter()
            .filter_map(|&b| Some(at(&o.accounts[b])? - o.tallies[b].first_lie_ms?))
            .max();
        ms.map_or(Value::Nothing, |ms| Value::Number(f64::from(ms) / 1000.0))
    };
    let past = accounts
        .iter()
        .map(|a| a.past_honest_yd)
        .fold(0.0, f32::max);
    let mine: Vec<usize> = bots.iter().filter_map(|&b| liar_of[b]).collect();
    let seen = o
        .tallies
        .iter()
        .flat_map(|t| mine.iter().map(|&l| t.seen[l]));
    let (mut positions, mut worst, mut misread, mut unaccepted) = (0, 0.0_f32, 0, 0);
    for s in seen {
        positions += s.positions;
        worst = worst.max(s.worst_yd);
        misread += s.misread;
        unaccepted += s.unaccepted;
    }
    let twins: Vec<usize> = spec
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.control_of == Some(gi))
        .flat_map(|(t, _)| (0..o.groups.len()).filter(move |&b| o.groups[b] == t))
        .collect();
    vec![
        ("lies", Value::Count(tallies.iter().map(|t| t.lies).sum())),
        ("lies_refused", sum(accounts, |a| a.lies_refused)),
        ("lies_stale", sum(accounts, |a| a.lies_stale)),
        ("lies_accepted", sum(accounts, |a| a.lies_accepted)),
        ("caught_after_s", after(|a| a.first_caught_ms)),
        ("through_after_s", after(|a| a.first_through_ms)),
        ("past_honest_yd", Value::Number(f64::from(past))),
        ("seen_positions", Value::Count(positions)),
        ("seen_past_honest_yd", Value::Number(f64::from(worst))),
        ("seen_misread", Value::Count(misread)),
        ("seen_unaccepted", Value::Count(unaccepted)),
        (
            "control_corrections",
            Value::Count(twins.iter().map(|&b| o.tallies[b].corrections).sum()),
        ),
        (
            "control_refused",
            Value::Count(
                twins
                    .iter()
                    .map(|&b| o.accounts[b].refused.iter().sum::<u64>())
                    .sum(),
            ),
        ),
    ]
}

/// The number an expectation reads: `GROUP.FIELD…` for a group's, otherwise the scenario's.
fn read<'a>(measured: &'a Value, spec: &Spec, e: &Expect) -> Option<&'a Value> {
    let path: Vec<&str> = e.path.split('.').collect();
    let is_group = spec
        .groups
        .iter()
        .any(|g| g.control_of.is_none() && g.name == path[0]);
    if is_group {
        measured.get("groups")?.number_at(&path)
    } else if path[0] == "groups" {
        None
    } else {
        measured.number_at(&path)
    }
}

/// Fails on an expectation that reads no number the verdict has.
pub fn check_paths(spec: &Spec) -> Result<(), Bad> {
    let zero = measured(spec, &Outcome::nothing(spec));
    for e in &spec.expects {
        if read(&zero, spec, e).is_none() {
            return Err(Bad::at(
                &e.at,
                format!("`{}` is not a number of the verdict", e.path),
            ));
        }
    }
    Ok(())
}

/// Each expectation, what it read, and whether it held.
pub fn judge(spec: &Spec, measured: &Value) -> Vec<(Expect, Value, bool)> {
    spec.expects
        .iter()
        .map(|e| {
            let got = read(measured, spec, e).cloned().unwrap_or(Value::Nothing);
            let held = e.op.holds(got.number(), e.want);
            (e.clone(), got, held)
        })
        .collect()
}

/// How the run went on this machine: its tick's threads, and the seconds it took to read the
/// scenario and load its ground.
pub struct Here {
    pub threads: usize,
    pub setup_s: f64,
}

/// The verdict line: what was measured, each expectation, and last how fast it ran here, which
/// no expectation reads.
pub fn line(measured: Value, judged: &[(Expect, Value, bool)], o: &Outcome, here: &Here) -> Value {
    let Value::Object(mut fields) = measured else {
        return measured;
    };
    let pass = judged.iter().all(|(_, _, ok)| *ok);
    fields.insert(1, ("pass".into(), Value::Yes(pass)));
    let expects = judged
        .iter()
        .map(|(e, got, ok)| {
            Value::object(vec![
                ("expect", Value::Text(e.to_string())),
                ("at", Value::Text(e.at.to_string())),
                ("got", got.clone()),
                ("pass", Value::Yes(*ok)),
            ])
        })
        .collect();
    fields.push(("expect".into(), Value::List(expects)));
    let game_s = o.ticks.len() as f64 * f64::from(o.tick_ms) / 1000.0;
    let summary = Summary::of(&o.ticks, here.threads, game_s, 0, 0);
    fields.extend([
        ("wall_s".into(), Value::Number(o.wall_s)),
        ("speedup".into(), Value::Number(game_s / o.wall_s.max(1e-9))),
        ("setup_s".into(), Value::Number(here.setup_s)),
        ("threads".into(), Value::Count(here.threads as u64)),
        (
            "tick_cpu_ms".into(),
            Value::List(vec![
                Value::Number(summary.cpu[0]),
                Value::Number(summary.cpu[1]),
            ]),
        ),
        ("load".into(), Value::Text(server::load_average())),
    ]);
    Value::Object(fields)
}
