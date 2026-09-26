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

/// The share of a zone's ground a texture must paint to count as the zone's.
const PAINTS_THE_ZONE: f64 = 0.001;

/// Ranks the catalog for one spot at a time, off the frame.
pub struct Ranker {
    pub catalog: Arc<Catalog>,
    around: Surroundings,
}

/// The lists for a spot.
pub struct Ranked {
    /// World X,Y, yards.
    pub at: [f32; 2],
    pub found: Found,
    /// Per tab of models, every kind first and then each of [`MODEL_KINDS`], the items best first.
    pub models: Vec<Vec<usize>>,
    /// Why each of the tables' models stands where it does.
    pub fits: Vec<Fit>,
    /// The ground textures best first, and why each of them stands where it does.
    pub ground: Vec<usize>,
    pub ground_why: BTreeMap<usize, String>,
    /// From the ray to the lists.
    pub took: Duration,
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
        let models = std::iter::once(None)
            .chain((0..MODEL_KINDS.len()).map(Some))
            .map(|kind| {
                evidence
                    .ranked(&score, kind)
                    .into_iter()
                    .filter_map(|m| catalog.item_of_model(m))
                    .collect()
            })
            .collect();
        let (ground, ground_why) = ground_order(&catalog, &found);
        Ok(Ranked {
            at,
            found,
            models,
            fits,
            ground,
            ground_why,
            took: asked.elapsed(),
        })
    }
}

/// The ground textures for a spot: the one under it, then those painted in the same chunks as that
/// one, most first, then the rest of the zone's, most first, then the others most painted first.
pub(super) fn ground_order(
    catalog: &Catalog,
    found: &Found,
) -> (Vec<usize>, BTreeMap<usize, String>) {
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
        .under
        .as_deref()
        .and_then(|t| catalog.find(t).filter(|&g| catalog.items[g].kind == GROUND));
    let mut why: BTreeMap<usize, String> = BTreeMap::new();
    let mut place: BTreeMap<usize, (u8, f64)> = BTreeMap::new();
    if let Some(u) = under {
        place.insert(u, (0, 0.0));
        why.insert(u, "under the spot".to_owned());
        let name = catalog.items[u].name().to_owned();
        for (other, share) in &catalog.items[u].beside {
            if let Some(&g) = by_name.get(&other.to_ascii_lowercase()) {
                place.entry(g).or_insert((1, -share));
                why.entry(g).or_insert_with(|| {
                    format!(
                        "{} of {name}'s ground lies in chunks that paint it too",
                        percent(*share)
                    )
                });
            }
        }
    }
    if let Some(zone) = zone {
        for g in grounds.clone() {
            let share = catalog.items[g]
                .zones
                .iter()
                .find(|(z, _)| z == zone)
                .map(|(_, s)| *s);
            let Some(share) = share.filter(|&s| s >= PAINTS_THE_ZONE) else {
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
    (order, why)
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
