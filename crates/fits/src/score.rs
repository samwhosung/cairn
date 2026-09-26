use std::collections::BTreeMap;

use crate::{AROUND, NEAR, Own, Tables};

/// Half a placement of every model, so that one never placed still has a chance.
const HALF: f64 = 0.5;
/// How many placements a ground's own counts must reach to outweigh the model's everywhere.
const GROUND_PRIOR: f64 = 50.0;
/// How many neighbours a model must be seen beside before they outweigh how common each is
/// everywhere: less, and a model placed a few times rides on a lucky pair or two.
const PAIR_PRIOR: f64 = 500.0;
/// More neighbours than this add no more certainty: a wood tells little more than a copse.
const MOST_NEIGHBOURS: f64 = 8.0;
/// A zone of its own's placement counts as this many of the install's.
pub const OWN_WEIGHT: f64 = 10.0;
/// Enough to put a model nothing has placed after every model something has.
const UNPLACED: f64 = 1.0e6;

/// Where a list is asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spot {
    /// The install's zone whose palette counts: the one the spot lies in, or the one a zone of its
    /// own borrows.
    pub zone: Option<usize>,
    /// The ground texture showing most there, and the slope's band.
    pub ground: Option<(usize, u8)>,
    /// What stands within [`AROUND`], by model, with its distance.
    pub near: Vec<(usize, f32)>,
}

impl Spot {
    /// The models standing within `within` yd and how many of each, or none.
    pub fn beside(&self, within: f32) -> Vec<(usize, u32)> {
        let mut by: BTreeMap<usize, u32> = BTreeMap::new();
        for &(m, d) in &self.near {
            if d <= within {
                *by.entry(m).or_default() += 1;
            }
        }
        by.into_iter().collect()
    }

    /// The neighbours a list weighs: those near when there are any, else those around.
    pub fn neighbours(&self) -> (f32, Vec<(usize, u32)>) {
        let near = self.beside(NEAR);
        if near.is_empty() {
            (AROUND, self.beside(AROUND))
        } else {
            (NEAR, near)
        }
    }
}

/// What counts toward a list: the install's placements, a zone of its own's, or both.
pub struct Evidence<'a> {
    pub tables: &'a Tables,
    pub own: &'a Own,
    install: f64,
    everywhere: Vec<f64>,
    placed: Vec<bool>,
}

/// A model on a list, and why it is there.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub model: usize,
    pub score: f64,
    /// How often the palette's zone places it, and the zone of its own.
    pub in_zone: u32,
    pub own: u32,
    /// How much more often it stands on the spot's ground and slope than anywhere.
    pub ground: Option<f64>,
    pub beside: Option<Beside>,
}

/// The neighbour that speaks most for a model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beside {
    pub model: usize,
    pub pairs: u32,
    pub within: f32,
    /// How far from such a neighbour the model's nearest usually stands.
    pub usual: Option<f32>,
}

impl<'a> Evidence<'a> {
    /// `install` false counts the zone's own placements alone.
    pub fn new(tables: &'a Tables, own: &'a Own, install: bool) -> Self {
        let install = if install { 1.0 } else { 0.0 };
        let mut placed: Vec<f64> = (0..tables.models.len())
            .map(|m| install * f64::from(tables.placed(m)))
            .collect();
        for (&m, &n) in &own.placed {
            if let Some(p) = placed.get_mut(m) {
                *p += OWN_WEIGHT * f64::from(n);
            }
        }
        let everywhere = smoothed(&placed);
        Self {
            tables,
            own,
            install,
            everywhere,
            placed: placed.iter().map(|&p| p > 0.0).collect(),
        }
    }

    /// Each model's share of everything placed: the list most placed first.
    pub fn everywhere(&self) -> Vec<f64> {
        self.everywhere.iter().map(|p| p.ln()).collect()
    }

    /// Each model's share of what the zone places, with the zone of its own's placements.
    pub fn palette(&self, zone: Option<usize>) -> Vec<f64> {
        let mut placed = vec![0.0; self.tables.models.len()];
        if let Some(list) = zone.and_then(|z| self.tables.palette.get(z)) {
            for &(m, n) in list {
                placed[m] += self.install * f64::from(n);
            }
        }
        add_own(&mut placed, self.own.placed.iter());
        if placed.iter().sum::<f64>() > 0.0 {
            smoothed(&placed).iter().map(|p| p.ln()).collect()
        } else {
            self.everywhere()
        }
    }

    /// How much more often each model stands on this ground and slope than anywhere, as a log.
    pub fn ground(&self, ground: (usize, u8)) -> Vec<f64> {
        let mut on = vec![0.0; self.tables.models.len()];
        if let Some(list) = self.tables.ground.get(&ground) {
            for &(m, n) in list {
                on[m] += self.install * f64::from(n);
            }
        }
        if let Some(list) = self.own.ground.get(&ground) {
            add_own(&mut on, list.iter());
        }
        let total: f64 = on.iter().sum();
        on.iter()
            .zip(&self.everywhere)
            .map(|(n, p)| ((n + GROUND_PRIOR * p) / (total + GROUND_PRIOR) / p).ln())
            .collect()
    }

    /// For each model, the log of how likely these neighbours are to stand beside it, within
    /// [`NEAR`] or [`AROUND`], weighed as no more than eight of them.
    pub fn beside(&self, within: f32, neighbours: &[(usize, u32)]) -> Vec<f64> {
        let m = self.tables.models.len();
        let r = usize::from(within > NEAR);
        let n: f64 = neighbours.iter().map(|&(_, k)| f64::from(k)).sum();
        if n == 0.0 {
            return vec![0.0; m];
        }
        let mut term: Vec<f64> = (0..m)
            .map(|b| {
                let sum = self.install * self.tables.index.sums[r][b] as f64
                    + OWN_WEIGHT * f64::from(self.own.sum(b, r));
                -n * (sum + PAIR_PRIOR).ln()
            })
            .collect();
        let mut with = vec![0.0; m];
        let mut touched = Vec::new();
        for &(a, k) in neighbours {
            for &(b, i) in &self.tables.index.beside[a] {
                let p = &self.tables.pairs[i];
                let pairs = [p.near, p.around][r] * if a == b { 2 } else { 1 };
                with[b] += self.install * f64::from(pairs);
                touched.push(b);
            }
            for (&(_, b), c) in self.own.pairs.range((a, 0)..(a + 1, 0)) {
                if b < m {
                    with[b] += OWN_WEIGHT * f64::from(c[r]);
                    touched.push(b);
                }
            }
            let floor = PAIR_PRIOR * self.everywhere[a];
            for &b in &touched {
                if with[b] > 0.0 {
                    term[b] += f64::from(k) * ((with[b] + floor).ln() - floor.ln());
                    with[b] = 0.0;
                }
            }
            touched.clear();
        }
        let scale = n.min(MOST_NEIGHBOURS) / n;
        for t in &mut term {
            *t *= scale;
        }
        term
    }

    /// The order a list takes at `spot`: its zone's palette, its ground and its neighbours, and
    /// last what nothing counted has placed on the ground, which none of them can speak for.
    pub fn scores(&self, spot: &Spot) -> Vec<f64> {
        let mut score = self.palette(spot.zone);
        if let Some(g) = spot.ground {
            add(&mut score, &self.ground(g));
        }
        let (within, neighbours) = spot.neighbours();
        add(&mut score, &self.beside(within, &neighbours));
        for (s, placed) in score.iter_mut().zip(&self.placed) {
            if !placed {
                *s -= UNPLACED;
            }
        }
        score
    }

    /// The first `top` models of `kind`, or of every kind, for `spot`, each with why it is there.
    pub fn list(&self, spot: &Spot, kind: Option<usize>, top: usize) -> Vec<Fit> {
        let score = self.scores(spot);
        let mut order: Vec<usize> = (0..score.len())
            .filter(|&m| kind.is_none_or(|k| self.tables.kind_of(m) == Some(k)))
            .collect();
        order.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));
        order.truncate(top);
        let lift = spot.ground.map(|g| self.ground(g));
        let (within, neighbours) = spot.neighbours();
        order
            .into_iter()
            .map(|m| Fit {
                model: m,
                score: score[m],
                in_zone: spot.zone.map_or(0, |z| self.tables.in_zone(z, m)),
                own: self.own.placed(m),
                ground: lift.as_ref().map(|l| l[m].exp()),
                beside: self.best_beside(m, within, &neighbours),
            })
            .collect()
    }

    /// Of the neighbours, the one seen beside `m` most often for how common both are.
    fn best_beside(&self, m: usize, within: f32, neighbours: &[(usize, u32)]) -> Option<Beside> {
        let r = usize::from(within > NEAR);
        let sum = |x: usize| {
            self.install * self.tables.index.sums[r][x] as f64
                + OWN_WEIGHT * f64::from(self.own.sum(x, r))
        };
        let total: f64 = (0..self.tables.models.len()).map(sum).sum();
        neighbours
            .iter()
            .filter_map(|&(a, _)| {
                let install = self
                    .tables
                    .pair(a, m)
                    .filter(|_| self.install > 0.0)
                    .map_or(0, |p| [p.near, p.around][r] * if a == m { 2 } else { 1 });
                let own = self.own.pairs.get(&(a, m)).map_or(0, |c| c[r]);
                let seen = self.install * f64::from(install) + OWN_WEIGHT * f64::from(own);
                (seen > 0.0).then(|| {
                    let lift = seen * total / (sum(a) * sum(m)).max(1.0);
                    (lift, a, install + own)
                })
            })
            .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)))
            .map(|(_, a, pairs)| Beside {
                model: a,
                pairs,
                within,
                usual: (self.install > 0.0)
                    .then(|| self.tables.usual(a, m))
                    .flatten()
                    .or_else(|| self.own.usual(a, m)),
            })
    }
}

fn add_own<'b>(counts: &mut [f64], own: impl Iterator<Item = (&'b usize, &'b u32)>) {
    for (&m, &n) in own {
        if let Some(c) = counts.get_mut(m) {
            *c += OWN_WEIGHT * f64::from(n);
        }
    }
}

/// Each count's share of the whole, with half a count more of every one.
fn smoothed(counts: &[f64]) -> Vec<f64> {
    let total: f64 = counts.iter().sum::<f64>() + HALF * counts.len() as f64;
    counts.iter().map(|c| (c + HALF) / total).collect()
}

pub(crate) fn add(to: &mut [f64], more: &[f64]) {
    for (a, b) in to.iter_mut().zip(more) {
        *a += b;
    }
}
