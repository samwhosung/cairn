use std::fmt::{self, Write};

use server::{Summary, Why};

use super::client::Tally;
use super::file::{Bad, Expect};
use super::played::Played;
use super::run::{Account, Outcome, liar_of};
use super::spec::Spec;

#[derive(Clone, Debug)]
pub enum Json {
    Count(u64),
    Whole(i64),
    Number(f64),
    Text(String),
    Yes(bool),
    NoNumber,
    Object(Vec<(String, Json)>),
    List(Vec<Json>),
}

impl Json {
    fn object(fields: Vec<(&str, Json)>) -> Self {
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

    fn number_at(&self, path: &[&str]) -> Option<&Self> {
        let mut v = self;
        for key in path {
            v = v.get(key)?;
        }
        matches!(
            v,
            Self::Count(_) | Self::Whole(_) | Self::Number(_) | Self::NoNumber
        )
        .then_some(v)
    }

    fn number(&self) -> Option<f64> {
        match *self {
            Self::Count(n) => Some(n as f64),
            Self::Whole(n) => Some(n as f64),
            Self::Number(n) => Some(n),
            _ => None,
        }
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Count(n) => write!(f, "{n}"),
            Self::Whole(n) => write!(f, "{n}"),
            Self::Number(n) => write!(f, "{n:.3}"),
            Self::Text(s) => write!(f, "\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            Self::Yes(b) => write!(f, "{b}"),
            Self::NoNumber => f.write_str("null"),
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

fn refused(counts: &[u64; Why::ALL.len()]) -> Json {
    let mut fields = vec![("all".to_string(), Json::Count(counts.iter().sum()))];
    for (why, &n) in Why::ALL.iter().zip(counts) {
        fields.push((format!("{why:?}").to_lowercase(), Json::Count(n)));
    }
    Json::Object(fields)
}

fn sum(accounts: &[&Account], f: impl Fn(&Account) -> u64) -> Json {
    Json::Count(accounts.iter().map(|a| f(a)).sum())
}

/// Everything the scenario measured that the same file measures the same on any machine.
pub fn measured(spec: &Spec, o: &Outcome) -> Json {
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
            ("bots", Json::Count(bots.len() as u64)),
            ("claims", sum(&accounts, |a| a.claims)),
            ("accepted", sum(&accounts, |a| a.accepted)),
            ("stale", sum(&accounts, |a| a.stale)),
            ("refused", refused(&refusals)),
            ("corrections", Json::Count(corrections)),
        ];
        if g.lie.is_some() {
            fields.extend(lies(o, &bots, &accounts, &liar_of, gi, spec));
        }
        if o.game.is_some() {
            let count =
                |f: fn(&Tally) -> u64| Json::Count(bots.iter().map(|&b| f(&o.tallies[b])).sum());
            fields.extend([
                ("actions", count(|t| t.actions)),
                ("roots", count(|t| t.roots)),
                ("placements", count(|t| t.placements)),
            ]);
        }
        groups.push((g.name.clone(), Json::object(fields)));
    }
    let all: Vec<&Account> = o.accounts.iter().collect();
    let mut refusals = [0; Why::ALL.len()];
    for a in &all {
        for (t, n) in refusals.iter_mut().zip(a.refused) {
            *t += n;
        }
    }
    let game_s = o.ticks.len() as f64 * f64::from(o.tick_ms) / 1000.0;
    let mut top = vec![
        ("scenario", Json::Text(spec.name.clone())),
        ("ticks", Json::Count(o.ticks.len() as u64)),
        ("game_s", Json::Number(game_s)),
        (
            "players",
            Json::Count(
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
            Json::Count(o.tallies.iter().map(|t| t.corrections).sum()),
        ),
        (
            "decode_errors",
            Json::Count(o.tallies.iter().map(|t| t.decode_errors).sum()),
        ),
        ("groups", Json::Object(groups)),
        (
            "hash",
            Json::Text(format!("{:016x}", o.ticks.last().map_or(0, |t| t.hash))),
        ),
    ];
    if let Some(game) = &o.game {
        top.push(("game", played(game)));
    }
    Json::object(top)
}

fn played(g: &Played) -> Json {
    let counted = g
        .counts
        .iter()
        .map(|&(what, n)| (what.to_string(), Json::Whole(n)))
        .collect();
    let first = |what: &Option<String>| what.clone().map_or(Json::NoNumber, Json::Text);
    Json::object(vec![
        ("name", Json::Text(g.name.to_string())),
        ("counted", Json::Object(counted)),
        ("hash_chain", Json::Text(format!("{:016x}", g.hash_chain))),
        ("saved_mismatches", Json::Count(g.saved_mismatches)),
        ("first_saved_mismatch", first(&g.first_saved_mismatch)),
        ("shown_mismatches", Json::Count(g.shown_mismatches)),
        ("first_shown_mismatch", first(&g.first_shown_mismatch)),
        ("plays_told", Json::Count(g.plays_told)),
        ("poses_told", Json::Count(g.poses_told)),
        ("idles_told", Json::Count(g.idles_told)),
        ("plays_out_of_view", Json::Count(g.plays_out_of_view)),
    ])
}

fn lies(
    o: &Outcome,
    bots: &[usize],
    accounts: &[&Account],
    liar_of: &[Option<usize>],
    gi: usize,
    spec: &Spec,
) -> Vec<(&'static str, Json)> {
    let tallies: Vec<_> = bots.iter().map(|&b| &o.tallies[b]).collect();
    let after = |at: fn(&Account) -> Option<u32>| {
        let ms = bots
            .iter()
            .filter_map(|&b| Some(at(&o.accounts[b])? - o.tallies[b].first_lie_ms?))
            .max();
        ms.map_or(Json::NoNumber, |ms| Json::Number(f64::from(ms) / 1000.0))
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
    let (mut kept_past_reach, mut shown_after) = (0, 0);
    for s in seen {
        positions += s.positions;
        worst = worst.max(s.worst_yd);
        misread += s.misread;
        unaccepted += s.unaccepted;
        kept_past_reach += s.kept_past_reach;
        shown_after = shown_after.max(s.shown_after_ticks);
    }
    let twins: Vec<usize> = spec
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.control_of == Some(gi))
        .flat_map(|(t, _)| (0..o.groups.len()).filter(move |&b| o.groups[b] == t))
        .collect();
    vec![
        ("lies", Json::Count(tallies.iter().map(|t| t.lies).sum())),
        ("lies_refused", sum(accounts, |a| a.lies_refused)),
        ("lies_stale", sum(accounts, |a| a.lies_stale)),
        ("lies_accepted", sum(accounts, |a| a.lies_accepted)),
        ("caught_after_s", after(|a| a.first_caught_ms)),
        ("through_after_s", after(|a| a.first_through_ms)),
        ("past_honest_yd", Json::Number(f64::from(past))),
        ("seen_positions", Json::Count(positions)),
        ("seen_past_honest_yd", Json::Number(f64::from(worst))),
        ("seen_misread", Json::Count(misread)),
        ("seen_unaccepted", Json::Count(unaccepted)),
        ("kept_past_reach", Json::Count(kept_past_reach)),
        ("shown_after_ticks", Json::Count(shown_after)),
        (
            "control_corrections",
            Json::Count(twins.iter().map(|&b| o.tallies[b].corrections).sum()),
        ),
        (
            "control_refused",
            Json::Count(
                twins
                    .iter()
                    .map(|&b| o.accounts[b].refused.iter().sum::<u64>())
                    .sum(),
            ),
        ),
    ]
}

fn read<'a>(measured: &'a Json, spec: &Spec, e: &Expect) -> Option<&'a Json> {
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
    let zero = measured(spec, &Outcome::zeroed(spec));
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

pub struct Judged {
    pub expect: Expect,
    pub got: Json,
    pub held: bool,
}

pub fn judge(spec: &Spec, measured: &Json) -> Vec<Judged> {
    spec.expects
        .iter()
        .map(|e| {
            let got = read(measured, spec, e).cloned().unwrap_or(Json::NoNumber);
            let held = e.op.holds(got.number(), e.want);
            Judged {
                expect: e.clone(),
                got,
                held,
            }
        })
        .collect()
}

pub struct ThisMachine {
    pub threads: usize,
    pub setup_s: f64,
}

/// The verdict line: what was measured, each expectation, and last how fast it ran here, which
/// no expectation reads.
pub fn line(measured: Json, judged: &[Judged], o: &Outcome, here: &ThisMachine) -> Json {
    let Json::Object(mut fields) = measured else {
        return measured;
    };
    let pass = judged.iter().all(|j| j.held);
    fields.insert(1, ("pass".into(), Json::Yes(pass)));
    let expects = judged
        .iter()
        .map(|j| {
            Json::object(vec![
                ("expect", Json::Text(j.expect.to_string())),
                ("at", Json::Text(j.expect.at.to_string())),
                ("got", j.got.clone()),
                ("pass", Json::Yes(j.held)),
            ])
        })
        .collect();
    fields.push(("expect".into(), Json::List(expects)));
    let game_s = o.ticks.len() as f64 * f64::from(o.tick_ms) / 1000.0;
    let summary = Summary::of(&o.ticks, here.threads, game_s, 0, 0);
    fields.extend([
        ("wall_s".into(), Json::Number(o.wall_s)),
        ("speedup".into(), Json::Number(game_s / o.wall_s.max(1e-9))),
        ("setup_s".into(), Json::Number(here.setup_s)),
        ("threads".into(), Json::Count(here.threads as u64)),
        (
            "tick_cpu_ms".into(),
            Json::List(vec![
                Json::Number(summary.cpu[0]),
                Json::Number(summary.cpu[1]),
            ]),
        ),
        ("load".into(), Json::Text(server::load_average())),
    ]);
    Json::Object(fields)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::scenario::file::{At, Setting, Text};
    use crate::scenario::spec::{RESERVED_GROUP_NAMES, spec};

    #[test]
    fn a_group_may_take_no_name_the_top_of_the_verdict_has() {
        let game = Setting {
            key: "game".into(),
            value: "melee".into(),
            at: At {
                file: "played.scenario".into(),
                line: 1,
            },
        };
        let text = Text {
            settings: vec![game],
            expects: Vec::new(),
        };
        let played = spec(text, Path::new("played.scenario")).expect("a spec");
        let Json::Object(top) = measured(&played, &Outcome::zeroed(&played)) else {
            panic!("the verdict is an object");
        };
        let keys: Vec<&str> = top.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, RESERVED_GROUP_NAMES);
    }
}
