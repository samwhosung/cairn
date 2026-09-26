use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::choice::{hash, hash_ignoring_case, key, unit_interval};
use crate::command::{PerModel, Scatter};
use crate::install::{Install, Rules};
use crate::mask::{Masking, Span};
use crate::text::{centi, scale_u16, two_places};
use crate::zone::{Id, Thing, Z, Zone};

const CANDIDATES_A_THING: f64 = 64.0;
const CANDIDATES_AT_MOST: i64 = 4_000_000;

struct Landed {
    at: [f64; 2],
    model_index: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub apart: f64,
    pub scale: (f64, f64),
    pub slope: Span,
    pub lean: bool,
    pub install_placed: Option<u32>,
}

pub struct Resolved {
    pub journaled: Scatter,
    pub rules: Vec<Rule>,
}

pub fn resolve(sc: &Scatter, install: &mut dyn Install) -> Result<Resolved, String> {
    fn pick<T: Copy + PartialEq>(rule: &PerModel<T>, i: usize) -> Option<T> {
        rule.given(i).copied()
    }
    let mut rules = Vec::with_capacity(sc.models.len());
    for (i, m) in sc.models.iter().enumerate() {
        let given = [
            pick(&sc.apart, i).is_some(),
            pick(&sc.scale, i).is_some(),
            pick(&sc.slope, i).is_some(),
            pick(&sc.lean, i).is_some(),
        ];
        let own: Option<Rules> = if given.contains(&false) {
            install.rules(m)?
        } else {
            None
        };
        let apart = pick(&sc.apart, i).or(own.map(|r| r.apart)).ok_or_else(|| {
            format!(
                "--apart D: no two closer than D yards; {m} has no rules to keep, as no \
                     catalog names it (`cairn zone --catalog DIR`, or $CAIRN_CATALOG, names the \
                     catalog)"
            )
        })?;
        let scale = pick(&sc.scale, i)
            .or(own.map(|r| (r.scale[0], r.scale[1])))
            .unwrap_or((1.0, 1.0));
        scale_u16(scale.0)?;
        scale_u16(scale.1)?;
        rules.push(Rule {
            apart,
            scale,
            slope: pick(&sc.slope, i)
                .or(own.map(|r| Span {
                    lo: Some(r.slope[0]),
                    hi: Some(r.slope[1]),
                }))
                .unwrap_or(Span::ANY_SLOPE),
            lean: pick(&sc.lean, i).or(own.map(|r| r.lean)).unwrap_or(false),
            install_placed: own.map(|r| r.placed),
        });
    }
    let journaled = Scatter {
        apart: PerModel::Each(rules.iter().map(|r| r.apart).collect()),
        scale: PerModel::Each(rules.iter().map(|r| r.scale).collect()),
        slope: PerModel::Each(rules.iter().map(|r| r.slope).collect()),
        lean: PerModel::Each(rules.iter().map(|r| r.lean).collect()),
        ..sc.clone()
    };
    Ok(Resolved { journaled, rules })
}

fn same_model(a: &str, b: &str) -> bool {
    a.replace('/', "\\")
        .eq_ignore_ascii_case(&b.replace('/', "\\"))
}

pub fn placed_by_same_seed(z: &Zone, sc: &Scatter) -> Vec<Id> {
    z.things
        .iter()
        .filter(|(_, t)| {
            t.scattered == Some(sc.seed)
                && sc.models.iter().any(|m| same_model(m, &t.model))
                && sc.area.reach(t.at()).is_some()
        })
        .map(|(id, _)| id.clone())
        .collect()
}

fn spots_key(seed: u64, models: &[String], aparts: &[f64], closest: f64) -> u64 {
    let mut words = vec![seed, hash_ignoring_case(&models.join("|"))];
    if aparts.iter().all(|a| a.to_bits() == closest.to_bits()) {
        words.push(closest.to_bits());
    } else {
        words.extend(aparts.iter().map(|a| a.to_bits()));
    }
    key(hash(&words))
}

fn grid_cell(closest: f64, [lo, hi]: [[f64; 2]; 2], count: usize) -> f64 {
    let each = (hi[0] - lo[0]) * (hi[1] - lo[1]) / (count.max(1) as f64 * CANDIDATES_A_THING);
    (closest / std::f64::consts::SQRT_2).max(each.sqrt())
}

pub fn spots(
    z: &Zone,
    sc: &Scatter,
    rules: &[Rule],
    masking: &Masking<'_>,
) -> Result<Vec<Thing>, String> {
    let aparts: Vec<f64> = rules.iter().map(|r| r.apart).collect();
    let closest = aparts.iter().copied().fold(f64::MAX, f64::min);
    let widest = aparts.iter().copied().fold(0.0, f64::max);
    if closest < fits::rules::LEAST_APART || !widest.is_finite() {
        return Err(format!(
            "--apart must be at least {} yd",
            fits::rules::LEAST_APART
        ));
    }
    let key = spots_key(sc.seed, &sc.models, &aparts, closest);
    let [lo, hi] = sc.area.bounds();
    let cell = grid_cell(closest, [lo, hi], sc.count);
    let (i0, i1) = ((lo[0] / cell).floor() as i64, (hi[0] / cell).ceil() as i64);
    let (j0, j1) = ((lo[1] / cell).floor() as i64, (hi[1] / cell).ceil() as i64);
    if (i1 - i0).saturating_mul(j1 - j0) > CANDIDATES_AT_MOST {
        return Err(format!(
            "the area holds too many {}-yd cells; scatter a smaller area or farther apart",
            two_places(cell)
        ));
    }
    let mut candidates = Vec::new();
    for j in j0..j1 {
        for i in i0..i1 {
            let h = |k: u64| hash(&[key, i as u64, j as u64, k]);
            let p = [
                (i as f64 + unit_interval(h(1))) * cell,
                (j as f64 + unit_interval(h(2))) * cell,
            ];
            if sc.area.reach(p).is_some() && z.frame.contains(p) {
                candidates.push((h(3), i, j, p));
            }
        }
    }
    candidates.sort_by_key(|c| (c.0, c.2, c.1));
    let mut taken: Vec<(i64, i64, [f64; 2], usize)> = Vec::new();
    let mut grid: BTreeMap<(i64, i64), Vec<Landed>> = BTreeMap::new();
    let g = widest.max(1e-6);
    for (_, i, j, p) in candidates {
        if taken.len() >= sc.count {
            break;
        }
        let h = |k: u64| hash(&[key, i as u64, j as u64, k]);
        let m = (h(4) % sc.models.len() as u64) as usize;
        let lets = masking.weight(z, p, Some(&rules[m].slope));
        if lets <= 0.0 || (lets < 1.0 && unit_interval(h(7)) >= lets) {
            continue;
        }
        let (gi, gj) = ((p[0] / g).floor() as i64, (p[1] / g).floor() as i64);
        let near = (-1..=1).any(|dj| {
            (-1..=1).any(|di| {
                grid.get(&(gi + di, gj + dj)).is_some_and(|v| {
                    v.iter().any(|l| {
                        let keep = f64::midpoint(aparts[m], aparts[l.model_index]);
                        (l.at[0] - p[0]).powi(2) + (l.at[1] - p[1]).powi(2) < keep * keep
                    })
                })
            })
        });
        if near {
            continue;
        }
        grid.entry((gi, gj)).or_default().push(Landed {
            at: p,
            model_index: m,
        });
        taken.push((i, j, p, m));
    }
    taken
        .into_iter()
        .map(|(i, j, p, m)| {
            let h = |k: u64| hash(&[key, i as u64, j as u64, k]);
            let r = &rules[m];
            let scale = r.scale.0 + (r.scale.1 - r.scale.0) * unit_interval(h(5));
            let facing = sc
                .facing
                .unwrap_or_else(|| (unit_interval(h(6)) * 36000.0).floor() / 100.0);
            Ok(Thing {
                model: sc.models[m].replace('/', "\\"),
                x: centi(p[0]),
                y: centi(p[1]),
                z: Z::Ground(0),
                facing: centi(facing.rem_euclid(360.0)),
                scale: scale_u16(scale)?,
                set: None,
                lean: r.lean,
                scattered: Some(sc.seed),
            })
        })
        .collect()
}

pub fn reply(
    z: &Zone,
    sc: &Scatter,
    rules: &[Rule],
    ids: &[Id],
    things: &[Thing],
    gone: usize,
) -> String {
    let mut msg = format!(
        "{} placed{}",
        ids.len(),
        match (ids.first(), ids.last()) {
            (Some(a), Some(b)) => format!(" (ids {a}..{b})"),
            _ => String::new(),
        }
    );
    if gone > 0 {
        let _ = write!(
            msg,
            "; the {gone} an earlier scatter with seed {} had placed there went",
            sc.seed
        );
    }
    if ids.len() < sc.count {
        let _ = write!(
            msg,
            "; only {} of {} fit where the rules and the mask let them in {} square yards",
            ids.len(),
            sc.count,
            two_places(sc.area.area())
        );
    }
    let own: Vec<String> = sc
        .models
        .iter()
        .zip(rules)
        .filter_map(|(m, r)| {
            let placed = r.install_placed?;
            let name = m.rsplit(['\\', '/']).next().unwrap_or(m);
            Some(format!(
                "{name} (placed {placed} times) on {}°, {} yd apart, scale {}..{}, {}",
                r.slope.text(),
                two_places(r.apart),
                two_places(r.scale.0),
                two_places(r.scale.1),
                if r.lean { "leaning" } else { "upright" }
            ))
        })
        .collect();
    if !own.is_empty() {
        let _ = write!(
            msg,
            "; the rules not given are where the install places each: {}",
            own.join("; ")
        );
    }
    let dupes = things
        .iter()
        .zip(ids)
        .filter(|(t, id)| {
            z.things.iter().any(|(other, o)| {
                other != *id
                    && (o.x - t.x).abs() <= 50
                    && (o.y - t.y).abs() <= 50
                    && o.model == t.model
            })
        })
        .count();
    if dupes > 0 {
        let _ = write!(
            msg,
            "; {dupes} stand within half a yard of the same model already there"
        );
    }
    msg
}
