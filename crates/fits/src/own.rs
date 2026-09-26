use std::collections::BTreeMap;

use crate::{AROUND, NEAR};

/// What a zone of its own has placed so far, counted as the tables count the install's.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Own {
    things: BTreeMap<String, Thing>,
    pub(crate) placed: BTreeMap<usize, u32>,
    pub(crate) ground: BTreeMap<(usize, u8), BTreeMap<usize, u32>>,
    /// Both ways round, and a model beside itself from both ends, near and around.
    pub(crate) pairs: BTreeMap<(usize, usize), [u32; 2]>,
    pub(crate) sums: BTreeMap<usize, [u32; 2]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Thing {
    model: usize,
    at: [f32; 2],
    ground: Option<(usize, u8)>,
}

impl Own {
    /// Places `model` at world `at` under the id `id`, standing on `ground`'s texture and slope
    /// band when known. An id already placed moves.
    pub fn place(&mut self, id: &str, model: usize, at: [f32; 2], ground: Option<(usize, u8)>) {
        self.remove(id);
        let thing = Thing { model, at, ground };
        self.pair_up(&thing, 1);
        *self.placed.entry(model).or_default() += 1;
        if let Some(g) = ground {
            *self.ground.entry(g).or_default().entry(model).or_default() += 1;
        }
        self.things.insert(id.to_owned(), thing);
    }

    /// Takes the thing with id `id` away, if there is one; says whether there was.
    pub fn remove(&mut self, id: &str) -> bool {
        let Some(thing) = self.things.remove(id) else {
            return false;
        };
        self.pair_up(&thing, -1);
        take(&mut self.placed, thing.model);
        if let Some(g) = thing.ground {
            let on = self.ground.entry(g).or_default();
            take(on, thing.model);
            if on.is_empty() {
                self.ground.remove(&g);
            }
        }
        true
    }

    /// The model and place of the thing with id `id`.
    pub fn get(&self, id: &str) -> Option<(usize, [f32; 2])> {
        self.things.get(id).map(|t| (t.model, t.at))
    }

    pub fn len(&self) -> usize {
        self.things.len()
    }

    pub fn is_empty(&self) -> bool {
        self.things.is_empty()
    }

    /// What stands within [`AROUND`] of `at`, by model, with its distance.
    pub fn around(&self, at: [f32; 2]) -> Vec<(usize, f32)> {
        let mut near: Vec<(usize, f32)> = self
            .things
            .values()
            .filter_map(|t| {
                let d = distance(t.at, at);
                (d <= AROUND).then_some((t.model, d))
            })
            .collect();
        near.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        near
    }

    /// How often the zone has placed `model`.
    pub fn placed(&self, model: usize) -> u32 {
        self.placed.get(&model).copied().unwrap_or(0)
    }

    /// Over every `from` with a `to` around it, the median distance to the nearest `to`.
    pub fn usual(&self, from: usize, to: usize) -> Option<f32> {
        let mut nearest: Vec<f32> = self
            .things
            .iter()
            .filter(|(_, t)| t.model == from)
            .filter_map(|(id, t)| {
                self.things
                    .iter()
                    .filter(|(other, o)| o.model == to && *other != id)
                    .map(|(_, o)| distance(t.at, o.at))
                    .filter(|&d| d <= AROUND)
                    .min_by(f32::total_cmp)
            })
            .collect();
        nearest.sort_by(f32::total_cmp);
        nearest.get(nearest.len().checked_sub(1)? / 2).copied()
    }

    fn pair_up(&mut self, thing: &Thing, by: i64) {
        for other in self.things.values() {
            let d = distance(thing.at, other.at);
            if d > AROUND {
                continue;
            }
            let (a, b) = (thing.model, other.model);
            let counts = [u32::from(d <= NEAR), 1];
            for key in [(a, b), (b, a)] {
                let pair = self.pairs.entry(key).or_default();
                for (c, n) in pair.iter_mut().zip(counts) {
                    *c = add(*c, n, by);
                }
                if *pair == [0, 0] {
                    self.pairs.remove(&key);
                }
            }
            for model in [a, b] {
                let sum = self.sums.entry(model).or_default();
                for (c, n) in sum.iter_mut().zip(counts) {
                    *c = add(*c, n, by);
                }
                if *sum == [0, 0] {
                    self.sums.remove(&model);
                }
            }
        }
    }

    /// How many placements `model` pairs with near (0) or around (1), itself counted from both ends.
    pub(crate) fn sum(&self, model: usize, within: usize) -> u32 {
        self.sums.get(&model).map_or(0, |s| s[within])
    }
}

fn add(count: u32, n: u32, by: i64) -> u32 {
    (i64::from(count) + by * i64::from(n)).max(0) as u32
}

fn take(counts: &mut BTreeMap<usize, u32>, model: usize) {
    if let Some(n) = counts.get_mut(&model) {
        *n -= 1;
        if *n == 0 {
            counts.remove(&model);
        }
    }
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
