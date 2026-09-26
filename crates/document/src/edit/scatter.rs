use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::choice::{hash, hash_ignoring_case, key, unit_interval};
use crate::command::Scatter;
use crate::text::{centi, scale_u16, two_places};
use crate::zone::{Id, Thing, Z, Zone};

pub fn spots(z: &Zone, sc: &Scatter) -> Result<Vec<Thing>, String> {
    if sc.apart < 0.5 || !sc.apart.is_finite() {
        return Err("--apart must be at least half a yard".into());
    }
    scale_u16(sc.scale.0)?;
    scale_u16(sc.scale.1)?;
    let key = key(hash(&[
        sc.seed,
        hash_ignoring_case(&sc.models.join("|")),
        sc.apart.to_bits(),
    ]));
    let cell = sc.apart / std::f64::consts::SQRT_2;
    let [lo, hi] = sc.area.bounds();
    let (i0, i1) = ((lo[0] / cell).floor() as i64, (hi[0] / cell).ceil() as i64);
    let (j0, j1) = ((lo[1] / cell).floor() as i64, (hi[1] / cell).ceil() as i64);
    if (i1 - i0).saturating_mul(j1 - j0) > 4_000_000 {
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
    let mut taken: Vec<(i64, i64, [f64; 2])> = Vec::new();
    let mut grid: BTreeMap<(i64, i64), Vec<[f64; 2]>> = BTreeMap::new();
    let g = sc.apart.max(1e-6);
    for (_, i, j, p) in candidates {
        if taken.len() >= sc.count {
            break;
        }
        let (gi, gj) = ((p[0] / g).floor() as i64, (p[1] / g).floor() as i64);
        let near = (-1..=1).any(|dj| {
            (-1..=1).any(|di| {
                grid.get(&(gi + di, gj + dj)).is_some_and(|v| {
                    v.iter().any(|q| {
                        (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2) < sc.apart * sc.apart
                    })
                })
            })
        });
        if near {
            continue;
        }
        grid.entry((gi, gj)).or_default().push(p);
        taken.push((i, j, p));
    }
    taken
        .into_iter()
        .map(|(i, j, p)| {
            let h = |k: u64| hash(&[key, i as u64, j as u64, k]);
            let model = sc.models[(h(4) % sc.models.len() as u64) as usize].replace('/', "\\");
            let scale = sc.scale.0 + (sc.scale.1 - sc.scale.0) * unit_interval(h(5));
            let facing = sc
                .facing
                .unwrap_or_else(|| (unit_interval(h(6)) * 36000.0).floor() / 100.0);
            Ok(Thing {
                model,
                x: centi(p[0]),
                y: centi(p[1]),
                z: Z::Ground(0),
                facing: centi(facing.rem_euclid(360.0)),
                scale: scale_u16(scale)?,
                set: None,
            })
        })
        .collect()
}

pub fn reply(z: &Zone, sc: &Scatter, ids: &[Id], things: &[Thing]) -> String {
    let mut msg = format!(
        "{} placed{}",
        ids.len(),
        match (ids.first(), ids.last()) {
            (Some(a), Some(b)) => format!(" (ids {a}..{b})"),
            _ => String::new(),
        }
    );
    if ids.len() < sc.count {
        let _ = write!(
            msg,
            "; only {} of {} fit {} yd apart in {} square yards",
            ids.len(),
            sc.count,
            two_places(sc.apart),
            two_places(sc.area.area())
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
            "; {dupes} stand within half a yard of the same model already there: the same \
             scatter lands in the same places, give --seed to vary it"
        );
    }
    msg
}
