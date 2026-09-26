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
pub use score::{Beside, Evidence, Fit, Spot, Why};
pub use survey::MODEL_KINDS;
pub use tally::{HEADER, Tally, shuffled};

/// Two things stand near each other within this many yards.
pub const NEAR: f32 = 8.0;
/// Two things stand around each other within this many yards.
pub const AROUND: f32 = 20.0;
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

/// How close two things stand to count as beside each other: within [`NEAR`] or [`AROUND`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Near,
    Around,
}

impl Reach {
    pub fn yards(self) -> f32 {
        match self {
            Self::Near => NEAR,
            Self::Around => AROUND,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts<T> {
    pub(crate) near: T,
    pub(crate) around: T,
}

impl<T: Copy> Counts<T> {
    pub(crate) fn get(self, reach: Reach) -> T {
        match reach {
            Reach::Near => self.near,
            Reach::Around => self.around,
        }
    }
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

impl Pair {
    /// How often a placement of either model has one of the other within `reach`: a model beside
    /// itself is seen from both ends of each pair.
    pub fn seen(&self, reach: Reach) -> u32 {
        let pairs = match reach {
            Reach::Near => self.near,
            Reach::Around => self.around,
        };
        if self.a == self.b { 2 * pairs } else { pairs }
    }
}

/// What the install places together: each zone's palette, what stands on each ground and slope,
/// and which models stand near which.
#[derive(Clone, Debug, PartialEq)]
pub struct Tables {
    pub models: Vec<Model>,
    pub grounds: Vec<String>,
    pub zones: Vec<Zone>,
    /// Per zone, each model it places and how often, in model order.
    pub palette: Vec<Vec<(usize, u32)>>,
    /// Per ground texture and slope band, each model standing there and how often, in model order.
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
    partners: Vec<Vec<Partner>>,
    seen_beside: Vec<Counts<u64>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Partner {
    model: usize,
    pair: usize,
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

    /// Where the model's kind stands in [`MODEL_KINDS`], or `None` for a kind no list is made of.
    pub fn kind_of(&self, model: usize) -> Option<usize> {
        let k = *self.index.kinds.get(model)?;
        (usize::from(k) < MODEL_KINDS.len()).then_some(usize::from(k))
    }

    /// How often the maps place `model` on the ground.
    pub fn placed(&self, model: usize) -> u32 {
        self.index.placed.get(model).copied().unwrap_or(0)
    }

    pub fn in_zone(&self, zone: usize, model: usize) -> u32 {
        self.palette.get(zone).map_or(0, |list| {
            list.binary_search_by_key(&model, |&(m, _)| m)
                .map_or(0, |i| list[i].1)
        })
    }

    /// The pair of `a` and `b`, when they stand around each other anywhere.
    pub fn pair(&self, a: usize, b: usize) -> Option<&Pair> {
        let list = self.index.partners.get(a)?;
        let i = list.binary_search_by_key(&b, |p| p.model).ok()?;
        self.pairs.get(list[i].pair)
    }

    /// How often a placement of `model` has another placement within `reach`.
    pub fn seen_beside(&self, model: usize, reach: Reach) -> u64 {
        self.index
            .seen_beside
            .get(model)
            .map_or(0, |c| c.get(reach))
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
                MODEL_KINDS
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
        let mut partners = vec![Vec::new(); m];
        let mut seen_beside = vec![Counts::<u64>::default(); m];
        for (pair, p) in t.pairs.iter().enumerate() {
            partners[p.a].push(Partner { model: p.b, pair });
            if p.a != p.b {
                partners[p.b].push(Partner { model: p.a, pair });
            }
            for model in [p.a, p.b] {
                seen_beside[model].near += u64::from(p.near);
                seen_beside[model].around += u64::from(p.around);
            }
        }
        for list in &mut partners {
            list.sort_unstable();
        }
        Self {
            kinds,
            placed,
            by_key,
            grounds_by_key,
            partners,
            seen_beside,
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
