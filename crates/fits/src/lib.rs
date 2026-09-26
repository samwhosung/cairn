//! What fits a spot: the models the install's maps place, ranked by the zone's palette, the ground
//! under the spot and what stands near it, each count taken from where Blizzard placed things.

mod count;
mod files;
mod own;
mod score;
mod tally;

use std::collections::BTreeMap;

pub use files::{FILES, read, write};
pub use own::Own;
pub use score::{Beside, Evidence, Fit, Spot};
pub use tally::{HEADER, Tally, shuffled};

/// Two things stand near each other within this many yards.
pub const NEAR: f32 = 8.0;
/// And around each other within this many.
pub const AROUND: f32 = 20.0;
/// What a list can be narrowed to, as the survey names a model's kind.
pub const KINDS: [&str; 6] = ["tree", "shrub", "rock", "fence", "prop", "building"];
const SLOPE_EDGES: [f32; 4] = [10.0, 20.0, 30.0, 45.0];
const SLOPE_NAMES: [&str; 5] = ["0-10", "10-20", "20-30", "30-45", "45-90"];

/// The band of slopes, counted from flat, that `degrees` lies in.
pub fn band(degrees: f32) -> u8 {
    SLOPE_EDGES
        .iter()
        .take_while(|&&edge| degrees >= edge)
        .count() as u8
}

/// A band's range of degrees, as `0-10`.
pub fn band_name(band: u8) -> &'static str {
    SLOPE_NAMES
        .get(usize::from(band))
        .copied()
        .unwrap_or("45-90")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    pub kind: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Zone {
    /// Its `Map.dbc` id.
    pub map: u32,
    /// Its `AreaTable` id, 0 for the ground of a map in no zone.
    pub area: u32,
    pub key: String,
    pub name: String,
}

/// One thing standing on a map's ground: what the tables are counted from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stand {
    pub model: usize,
    pub zone: usize,
    /// World x (north) and y (west), yards.
    pub at: [f32; 2],
    /// The ground texture showing most under it, and its slope's band.
    pub ground: Option<(usize, u8)>,
}

/// Two models that stand within [`AROUND`] of each other somewhere, `a` no later than `b`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub a: usize,
    pub b: usize,
    /// Pairs of placements within [`NEAR`].
    pub near: u32,
    /// Pairs of placements within [`AROUND`], the near ones among them.
    pub around: u32,
    /// Over every `a` with a `b` around it, the median distance to the nearest `b`.
    pub a_to_b: f32,
    pub b_to_a: f32,
}

/// What the install places together: each zone's palette, what stands on each ground and slope,
/// and which models stand near which.
#[derive(Clone, Debug, PartialEq)]
pub struct Tables {
    pub models: Vec<Model>,
    pub grounds: Vec<String>,
    pub zones: Vec<Zone>,
    /// Per zone, each model it places and how often, by model.
    pub palette: Vec<Vec<(usize, u32)>>,
    /// Per ground texture and slope band, each model standing there and how often, by model.
    pub ground: BTreeMap<(usize, u8), Vec<(usize, u32)>>,
    pub pairs: Vec<Pair>,
    index: Index,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Index {
    kinds: Vec<u8>,
    placed: Vec<u32>,
    by_key: BTreeMap<String, usize>,
    grounds_by_key: BTreeMap<String, usize>,
    /// Per model, the models it pairs with, by model, and the pair's place in `pairs`.
    beside: Vec<Vec<(usize, usize)>>,
    /// Per model, near and around: how many placements it pairs with, a pair of it with itself
    /// counted from both ends.
    sums: [Vec<u64>; 2],
}

impl Tables {
    /// Counted from every placement the survey found.
    pub fn of(survey: &survey::Survey) -> Self {
        let (models, grounds, zones) = lists(survey);
        Self::count(models, grounds, zones, &stands(survey))
    }

    pub fn count(
        models: Vec<Model>,
        grounds: Vec<String>,
        zones: Vec<Zone>,
        stands: &[Stand],
    ) -> Self {
        count::count(models, grounds, zones, stands)
    }

    fn from_parts(
        models: Vec<Model>,
        grounds: Vec<String>,
        zones: Vec<Zone>,
        palette: Vec<Vec<(usize, u32)>>,
        ground: BTreeMap<(usize, u8), Vec<(usize, u32)>>,
        pairs: Vec<Pair>,
    ) -> Self {
        let mut tables = Self {
            models,
            grounds,
            zones,
            palette,
            ground,
            pairs,
            index: Index::default(),
        };
        tables.index = Index::of(&tables);
        tables
    }

    /// The model at an install path, in any case, with `/` or `\` and either extension.
    pub fn model(&self, path: &str) -> Option<usize> {
        self.index.by_key.get(&survey::key(path)).copied()
    }

    pub fn ground_texture(&self, path: &str) -> Option<usize> {
        self.index.grounds_by_key.get(&survey::key(path)).copied()
    }

    /// The zone of `AreaTable` id `area` on map `map`, or the map's ground in no zone for 0.
    pub fn zone(&self, map: u32, area: u32) -> Option<usize> {
        self.zones
            .iter()
            .position(|z| z.map == map && z.area == area)
    }

    /// The zone named `name` in any case, as `AreaTable` picks one: the lowest id, and of a zone
    /// whose ground lies on two maps, the map it places most on.
    pub fn zone_named(&self, name: &str) -> Option<usize> {
        let placed = |z: usize| {
            self.palette[z]
                .iter()
                .map(|&(_, n)| u64::from(n))
                .sum::<u64>()
        };
        (0..self.zones.len())
            .filter(|&z| self.zones[z].area != 0 && self.zones[z].name.eq_ignore_ascii_case(name))
            .min_by_key(|&z| (self.zones[z].area, std::cmp::Reverse(placed(z)), z))
    }

    /// Where the model's kind stands in [`KINDS`], or `None` for a kind no list is made of.
    pub fn kind_of(&self, model: usize) -> Option<usize> {
        let k = *self.index.kinds.get(model)?;
        (usize::from(k) < KINDS.len()).then_some(usize::from(k))
    }

    /// How often the maps place `model` on the ground.
    pub fn placed(&self, model: usize) -> u32 {
        self.index.placed.get(model).copied().unwrap_or(0)
    }

    /// How often the zone places `model`.
    pub fn in_zone(&self, zone: usize, model: usize) -> u32 {
        self.palette.get(zone).map_or(0, |list| {
            list.binary_search_by_key(&model, |&(m, _)| m)
                .map_or(0, |i| list[i].1)
        })
    }

    /// The pair of `a` and `b`, when they stand around each other anywhere.
    pub fn pair(&self, a: usize, b: usize) -> Option<&Pair> {
        let list = self.index.beside.get(a)?;
        let i = list.binary_search_by_key(&b, |&(m, _)| m).ok()?;
        self.pairs.get(list[i].1)
    }

    /// How far the nearest `to` usually stands from a `from` that has one around it.
    pub fn usual(&self, from: usize, to: usize) -> Option<f32> {
        let p = self.pair(from, to)?;
        Some(if p.a == from { p.a_to_b } else { p.b_to_a })
    }
}

impl Index {
    fn of(t: &Tables) -> Self {
        let m = t.models.len();
        let kinds = t
            .models
            .iter()
            .map(|model| {
                KINDS
                    .iter()
                    .position(|k| *k == model.kind)
                    .map_or(u8::MAX, |k| k as u8)
            })
            .collect();
        let mut placed = vec![0u32; m];
        for &(model, n) in t.palette.iter().flatten() {
            placed[model] += n;
        }
        let by_key = t
            .models
            .iter()
            .enumerate()
            .map(|(i, model)| (survey::key(&model.path), i))
            .collect();
        let grounds_by_key = t
            .grounds
            .iter()
            .enumerate()
            .map(|(i, g)| (survey::key(g), i))
            .collect();
        let mut beside = vec![Vec::new(); m];
        let mut sums = [vec![0u64; m], vec![0u64; m]];
        for (i, p) in t.pairs.iter().enumerate() {
            beside[p.a].push((p.b, i));
            if p.a != p.b {
                beside[p.b].push((p.a, i));
            }
            for (sum, n) in sums.iter_mut().zip([p.near, p.around]) {
                sum[p.a] += u64::from(n);
                sum[p.b] += u64::from(n);
            }
        }
        for list in &mut beside {
            list.sort_unstable();
        }
        Self {
            kinds,
            placed,
            by_key,
            grounds_by_key,
            beside,
            sums,
        }
    }
}

/// The survey's models, grounds and zones, which a count of only some of its placements needs.
pub fn lists(survey: &survey::Survey) -> (Vec<Model>, Vec<String>, Vec<Zone>) {
    let models = survey
        .models
        .iter()
        .map(|m| Model {
            kind: m.kind.to_owned(),
            path: m.path.clone(),
        })
        .collect();
    let grounds = survey.grounds.iter().map(|g| g.path.clone()).collect();
    let zones = survey
        .zones
        .iter()
        .map(|z| Zone {
            map: z.map,
            area: z.area,
            key: z.key.clone(),
            name: z.name.clone(),
        })
        .collect();
    (models, grounds, zones)
}

/// Every placement the survey found, as the tables count it.
pub fn stands(survey: &survey::Survey) -> Vec<Stand> {
    survey
        .placements
        .iter()
        .map(|p| Stand {
            model: p.model,
            zone: p.zone,
            at: [p.position[0], p.position[1]],
            ground: p.ground.zip(p.slope.map(band)),
        })
        .collect()
}

#[cfg(test)]
mod tests;
