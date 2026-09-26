use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fits::{Fit, MODEL_KINDS};
use world::coords::bevy_to_wow;
use world::sight::Cast;
use world::{CurrentMap, Install};

use super::catalog::{Catalog, GROUND};
use crate::args;
use crate::catalog::what_fits::{Found, Surroundings};
use crate::zone::Zone;

const LEAST_ZONE_SHARE: f64 = 0.001;

pub struct Ranker {
    pub catalog: Arc<Catalog>,
    around: Surroundings,
}

pub struct Ranked {
    /// World X,Y, yards.
    pub at: [f32; 2],
    pub found: Found,
    pub models: ModelOrder,
    /// Why each of the tables' models stands where it does.
    pub fits: Vec<Fit>,
    pub ground: GroundOrder,
    /// From the ray to the lists.
    pub took: Duration,
}

/// The model items, best first.
pub struct ModelOrder {
    pub every_kind: Vec<usize>,
    pub of_kind: [Vec<usize>; MODEL_KINDS.len()],
}

/// The ground items, best first, and why each stands where it does.
pub struct GroundOrder {
    pub order: Vec<usize>,
    pub why: BTreeMap<usize, String>,
}

impl Ranker {
    pub fn open(
        dir: &Path,
        install: Install,
        map: &CurrentMap,
        opened: &args::Map,
    ) -> Result<Self, String> {
        let catalog = Catalog::read(dir)?;
        let around = match opened {
            args::Map::Install { .. } => Surroundings::on_the_install(install, map)?,
            args::Map::Zone(root) => {
                let zone = Zone::read(root)?;
                Surroundings::of_a_zone_of_its_own(&catalog.tables, &install, &zone, None)?
            }
        };
        Ok(Self {
            catalog: Arc::new(catalog),
            around,
        })
    }

    /// The lists for what the ray meets first, or nothing when it meets nothing.
    pub fn rank(&mut self, ray: Cast, asked: Instant) -> Result<Option<Ranked>, String> {
        let Some(met) = ray.first() else {
            return Ok(None);
        };
        let [x, y, _] = bevy_to_wow(met.point);
        self.rank_at([x, y], asked).map(Some)
    }

    pub fn rank_at(&mut self, at: [f32; 2], asked: Instant) -> Result<Ranked, String> {
        let catalog = Arc::clone(&self.catalog);
        let tables = &catalog.tables;
        let found = self.around.find(tables, at)?;
        let evidence = self.around.evidence(tables);
        let score = evidence.scores(&found.spot);
        let why = evidence.explain(&found.spot);
        let fits = (0..score.len()).map(|m| why.fit(m, score[m])).collect();
        let items = |kind: Option<usize>| -> Vec<usize> {
            evidence
                .ranked(&score, kind)
                .into_iter()
                .filter_map(|m| catalog.item_of_model(m))
                .collect()
        };
        let models = ModelOrder {
            every_kind: items(None),
            of_kind: std::array::from_fn(|k| items(Some(k))),
        };
        let ground = ground_order(&catalog, &found);
        Ok(Ranked {
            at,
            found,
            models,
            fits,
            ground,
            took: asked.elapsed(),
        })
    }
}

/// The ground textures for a spot: the one under it, then those painted in the same chunks as that
/// one, most first, then the rest of the zone's, most first, then the others most painted first.
pub(super) fn ground_order(catalog: &Catalog, found: &Found) -> GroundOrder {
    let grounds = catalog.models..catalog.items.len();
    let by_name: BTreeMap<String, usize> = grounds
        .clone()
        .map(|g| (catalog.items[g].name().to_ascii_lowercase(), g))
        .collect();
    let zone = found
        .spot
        .zone
        .map(|z| catalog.tables.zones[z].name.as_str());
    let under = found
        .texture
        .as_deref()
        .and_then(|t| catalog.find(t).filter(|&g| catalog.items[g].kind == GROUND));
    let mut why: BTreeMap<usize, String> = BTreeMap::new();
    let mut place: BTreeMap<usize, (u8, f64)> = BTreeMap::new();
    if let Some(u) = under {
        place.insert(u, (0, 0.0));
        why.insert(u, "under the spot".to_owned());
        let name = catalog.items[u].name().to_owned();
        let beside = catalog.items[u].painted.iter().flat_map(|p| &p.beside);
        for other in beside {
            if let Some(&g) = by_name.get(&other.name.to_ascii_lowercase()) {
                place.entry(g).or_insert((1, -other.share));
                why.entry(g).or_insert_with(|| {
                    format!(
                        "{} of {name}'s ground lies in chunks that paint it too",
                        percent(other.share)
                    )
                });
            }
        }
    }
    if let Some(zone) = zone {
        for g in grounds.clone() {
            let share = catalog.items[g]
                .painted
                .iter()
                .flat_map(|p| &p.in_zones)
                .find(|s| s.name == zone)
                .map(|s| s.share);
            let Some(share) = share.filter(|&s| s >= LEAST_ZONE_SHARE) else {
                continue;
            };
            place.entry(g).or_insert((2, -share));
            let said = format!("{zone} paints {} of its ground with it", percent(share));
            why.entry(g)
                .and_modify(|w| {
                    w.push_str("; ");
                    w.push_str(&said);
                })
                .or_insert(said);
        }
    }
    let mut order: Vec<usize> = grounds.collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (place.get(&a), place.get(&b));
        match (pa, pb) {
            (Some(x), Some(y)) => x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
        .then_with(|| catalog.plainly(a, b))
    });
    GroundOrder { order, why }
}

fn percent(share: f64) -> String {
    let p = 100.0 * share;
    if p >= 9.95 {
        format!("{p:.0}%")
    } else if p >= 0.095 {
        format!("{p:.1}%")
    } else {
        "under 0.1%".to_owned()
    }
}
