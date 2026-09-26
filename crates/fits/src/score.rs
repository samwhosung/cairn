use std::collections::BTreeMap;

use crate::{Own, Reach, Tables};

const PSEUDOCOUNT: f64 = 0.5;
const GROUND_PSEUDOCOUNT: f64 = 50.0;
const PAIR_PSEUDOCOUNT: f64 = 500.0;
const MOST_NEIGHBOURS: f64 = 8.0;
const OWN_WEIGHT: f64 = 10.0;
const UNPLACED_PENALTY: f64 = 1.0e6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spot {
    /// The install's zone whose palette counts: the one the spot lies in, or the one a zone of its
    /// own borrows.
    pub zone: Option<usize>,
    /// The ground texture showing most there, and the slope's band.
    pub ground: Option<(usize, u8)>,
    /// What stands within [`crate::AROUND`]: each thing's model and distance, nearest first.
    pub near: Vec<(usize, f32)>,
}

impl Spot {
    /// The models standing within `reach` and how many of each, in model order.
    pub fn beside(&self, reach: Reach) -> Vec<(usize, u32)> {
        let mut by: BTreeMap<usize, u32> = BTreeMap::new();
        for &(m, d) in &self.near {
            if d <= reach.yards() {
                *by.entry(m).or_default() += 1;
            }
        }
        by.into_iter().collect()
    }

    /// The neighbours a list weighs: those near when there are any, else those around.
    pub fn neighbours(&self) -> (Reach, Vec<(usize, u32)>) {
        let near = self.beside(Reach::Near);
        if near.is_empty() {
            (Reach::Around, self.beside(Reach::Around))
        } else {
            (Reach::Near, near)
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
    /// How often the palette's zone places it.
    pub in_zone: u32,
    /// How often the zone of its own has placed it.
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
    pub reach: Reach,
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
        add_own(&mut placed, own.placed.iter());
        let everywhere = smoothed(&placed);
        Self {
            tables,
            own,
            install,
            everywhere,
            placed: placed.iter().map(|&p| p > 0.0).collect(),
        }
    }

    /// The log of each model's share of everything placed: the list most placed first.
    pub fn everywhere(&self) -> Vec<f64> {
        self.everywhere.iter().map(|p| p.ln()).collect()
    }

    /// The log of each model's share of what the zone places, with the zone of its own's
    /// placements; of everything placed, when neither has placed anything.
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
            .map(|(n, p)| ((n + GROUND_PSEUDOCOUNT * p) / (total + GROUND_PSEUDOCOUNT) / p).ln())
            .collect()
    }

    /// For each model, the log of how likely these neighbours are to stand within `reach` of it,
    /// weighed as no more than eight of them.
    pub fn beside(&self, reach: Reach, neighbours: &[(usize, u32)]) -> Vec<f64> {
        let m = self.tables.models.len();
        let n: f64 = neighbours.iter().map(|&(_, k)| f64::from(k)).sum();
        if n == 0.0 {
            return vec![0.0; m];
        }
        let mut term: Vec<f64> = (0..m)
            .map(|b| -n * (self.seen_beside(b, reach) + PAIR_PSEUDOCOUNT).ln())
            .collect();
        let mut with = vec![0.0; m];
        let mut touched = Vec::new();
        for &(a, k) in neighbours {
            for partner in &self.tables.index.partners[a] {
                let seen = self.tables.pairs[partner.pair].seen(reach);
                with[partner.model] += self.install * f64::from(seen);
                touched.push(partner.model);
            }
            for (&(_, b), counts) in self.own.pairs_both_ways.range((a, 0)..(a + 1, 0)) {
                if b < m {
                    with[b] += OWN_WEIGHT * f64::from(counts.get(reach));
                    touched.push(b);
                }
            }
            let floor = PAIR_PSEUDOCOUNT * self.everywhere[a];
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
        let (reach, neighbours) = spot.neighbours();
        add(&mut score, &self.beside(reach, &neighbours));
        for (s, placed) in score.iter_mut().zip(&self.placed) {
            if !placed {
                *s -= UNPLACED_PENALTY;
            }
        }
        score
    }

    /// The first `top` models of `kind`, or of every kind, for `spot`, each with why it is there.
    pub fn list(&self, spot: &Spot, kind: Option<usize>, top: usize) -> Vec<Fit> {
        let score = self.scores(spot);
        let mut order = self.ranked(&score, kind);
        order.truncate(top);
        let why = self.explain(spot);
        order.into_iter().map(|m| why.fit(m, score[m])).collect()
    }

    /// Every model of `kind`, or of every kind, in the order a list takes by `score`: the highest
    /// first, and on a tie the earlier model.
    pub fn ranked(&self, score: &[f64], kind: Option<usize>) -> Vec<usize> {
        let mut order: Vec<usize> = (0..score.len())
            .filter(|&m| kind.is_none_or(|k| self.tables.kind_of(m) == Some(k)))
            .collect();
        order.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));
        order
    }

    pub fn explain(&'a self, spot: &'a Spot) -> Why<'a> {
        let (reach, neighbours) = spot.neighbours();
        let seen_beside = (0..self.tables.models.len())
            .map(|x| self.seen_beside(x, reach))
            .sum();
        Why {
            evidence: self,
            spot,
            lift: spot.ground.map(|g| self.ground(g)),
            reach,
            neighbours,
            seen_beside,
        }
    }

    fn best_beside(
        &self,
        m: usize,
        reach: Reach,
        neighbours: &[(usize, u32)],
        total: f64,
    ) -> Option<Beside> {
        neighbours
            .iter()
            .filter_map(|&(a, _)| {
                let install = self
                    .tables
                    .pair(a, m)
                    .filter(|_| self.install > 0.0)
                    .map_or(0, |p| p.seen(reach));
                let own = self
                    .own
                    .pairs_both_ways
                    .get(&(a, m))
                    .map_or(0, |c| c.get(reach));
                let seen = self.install * f64::from(install) + OWN_WEIGHT * f64::from(own);
                (seen > 0.0).then(|| {
                    let lift = pair_lift(
                        seen,
                        total,
                        self.seen_beside(a, reach),
                        self.seen_beside(m, reach),
                    );
                    (lift, a, install + own)
                })
            })
            .max_by(|x, y| x.0.total_cmp(&y.0).then(y.1.cmp(&x.1)))
            .map(|(_, a, pairs)| Beside {
                model: a,
                pairs,
                reach,
                usual: (self.install > 0.0)
                    .then(|| self.tables.usual(a, m))
                    .flatten()
                    .or_else(|| self.own.usual(a, m)),
            })
    }

    fn seen_beside(&self, model: usize, reach: Reach) -> f64 {
        self.install * self.tables.seen_beside(model, reach) as f64
            + OWN_WEIGHT * f64::from(self.own.seen_beside(model, reach))
    }
}

/// Why each model stands where it does on the lists for one spot.
pub struct Why<'a> {
    evidence: &'a Evidence<'a>,
    spot: &'a Spot,
    lift: Option<Vec<f64>>,
    reach: Reach,
    neighbours: Vec<(usize, u32)>,
    seen_beside: f64,
}

impl Why<'_> {
    pub fn fit(&self, m: usize, score: f64) -> Fit {
        let e = self.evidence;
        Fit {
            model: m,
            score,
            in_zone: self.spot.zone.map_or(0, |z| e.tables.in_zone(z, m)),
            own: e.own.placed(m),
            ground: self.lift.as_ref().map(|l| l[m].exp()),
            beside: e.best_beside(m, self.reach, &self.neighbours, self.seen_beside),
        }
    }
}

fn pair_lift(seen: f64, total: f64, a: f64, b: f64) -> f64 {
    seen * total / (a * b).max(1.0)
}

fn add_own<'b>(counts: &mut [f64], own: impl Iterator<Item = (&'b usize, &'b u32)>) {
    for (&m, &n) in own {
        if let Some(c) = counts.get_mut(m) {
            *c += OWN_WEIGHT * f64::from(n);
        }
    }
}

fn smoothed(counts: &[f64]) -> Vec<f64> {
    let total: f64 = counts.iter().sum::<f64>() + PSEUDOCOUNT * counts.len() as f64;
    counts.iter().map(|c| (c + PSEUDOCOUNT) / total).collect()
}

pub(crate) fn add(to: &mut [f64], more: &[f64]) {
    for (a, b) in to.iter_mut().zip(more) {
        *a += b;
    }
}
