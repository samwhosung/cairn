use std::collections::{BTreeMap, HashMap};

use crate::{AROUND, Model, NEAR, Pair, Stand, Tables, Zone};

type Cell = (u32, i32, i32);

pub(crate) fn count(
    models: Vec<Model>,
    grounds: Vec<String>,
    zones: Vec<Zone>,
    stands: &[Stand],
) -> Tables {
    let mut palette: Vec<BTreeMap<usize, u32>> = vec![BTreeMap::new(); zones.len()];
    let mut ground: BTreeMap<(usize, u8), BTreeMap<usize, u32>> = BTreeMap::new();
    for s in stands {
        *palette[s.zone].entry(s.model).or_default() += 1;
        if let Some(g) = s.ground {
            *ground.entry(g).or_default().entry(s.model).or_default() += 1;
        }
    }
    let pairs = pairs(&zones, stands);
    Tables::from_parts(
        models,
        grounds,
        zones,
        palette
            .into_iter()
            .map(|z| z.into_iter().collect())
            .collect(),
        ground
            .into_iter()
            .map(|(g, on)| (g, on.into_iter().collect()))
            .collect(),
        pairs,
    )
}

fn cell(map: u32, at: [f32; 2]) -> Cell {
    (
        map,
        (at[0] / AROUND).floor() as i32,
        (at[1] / AROUND).floor() as i32,
    )
}

fn pairs(zones: &[Zone], stands: &[Stand]) -> Vec<Pair> {
    let mut grid: HashMap<Cell, Vec<usize>> = HashMap::new();
    for (i, s) in stands.iter().enumerate() {
        grid.entry(cell(zones[s.zone].map, s.at))
            .or_default()
            .push(i);
    }
    let mut counts: HashMap<(usize, usize), [u32; 2]> = HashMap::new();
    let mut nearest: HashMap<(usize, usize), Vec<f32>> = HashMap::new();
    let mut here: Vec<(usize, f32)> = Vec::new();
    for (i, s) in stands.iter().enumerate() {
        let (map, cx, cy) = cell(zones[s.zone].map, s.at);
        here.clear();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for &j in grid.get(&(map, cx + dx, cy + dy)).into_iter().flatten() {
                    if j == i {
                        continue;
                    }
                    let o = &stands[j];
                    let d = (s.at[0] - o.at[0]).hypot(s.at[1] - o.at[1]);
                    if d > AROUND {
                        continue;
                    }
                    here.push((o.model, d));
                    if j > i {
                        let key = (s.model.min(o.model), s.model.max(o.model));
                        let c = counts.entry(key).or_default();
                        c[0] += u32::from(d <= NEAR);
                        c[1] += 1;
                    }
                }
            }
        }
        here.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        here.dedup_by_key(|n| n.0);
        for &(other, d) in &here {
            nearest.entry((s.model, other)).or_default().push(d);
        }
    }
    let mut keys: Vec<(usize, usize)> = counts.keys().copied().collect();
    keys.sort_unstable();
    keys.into_iter()
        .map(|(a, b)| {
            let [near, around] = counts[&(a, b)];
            Pair {
                a,
                b,
                near,
                around,
                a_to_b: to_tenth(lower_median(nearest.get_mut(&(a, b)))),
                b_to_a: to_tenth(lower_median(nearest.get_mut(&(b, a)))),
            }
        })
        .collect()
}

fn lower_median(distances: Option<&mut Vec<f32>>) -> f32 {
    let Some(d) = distances.filter(|d| !d.is_empty()) else {
        return f32::NAN;
    };
    d.sort_by(f32::total_cmp);
    d[(d.len() - 1) / 2]
}

fn to_tenth(yards: f32) -> f32 {
    (yards * 10.0).round() / 10.0
}
