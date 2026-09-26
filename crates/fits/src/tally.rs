use std::cmp::Ordering;
use std::fmt::Write as _;

use crate::Tables;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

/// A list in an order that knows nothing of the spot, fixed by a hash of each model's path: the
/// control every list must beat.
pub fn shuffled(tables: &Tables) -> Vec<f64> {
    tables
        .models
        .iter()
        .map(|m| (fnv(m.path.bytes()) >> 11) as f64)
        .collect()
}

pub(crate) fn fnv(bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(FNV_OFFSET, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    })
}

/// The columns of [`Tally::row`].
pub const HEADER: &str = "list order                                   ranked  hit@1 hit@10 hit@20   MRR | its kind: first  first 5  first page (20)";

/// Where one list order put each model that was placed, among every model and among its kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub name: String,
    places: Vec<(u32, u32)>,
}

impl Tally {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            places: Vec::new(),
        }
    }

    /// Counts where `score`, highest first and the earlier model on a tie, puts `placed`.
    pub fn add(&mut self, tables: &Tables, score: &[f64], placed: usize) {
        let kind = tables.kind_of(placed);
        let s = score[placed];
        let (mut all, mut in_kind) = (1, 1);
        for (m, x) in score.iter().enumerate() {
            let ahead = match x.total_cmp(&s) {
                Ordering::Greater => true,
                Ordering::Equal => m < placed,
                Ordering::Less => false,
            };
            if ahead {
                all += 1;
                if tables.kind_of(m) == kind {
                    in_kind += 1;
                }
            }
        }
        self.places.push((all, in_kind));
    }

    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// The share of placements the list put within the first `n` of every model, or of its kind.
    pub fn hit(&self, n: u32, in_kind: bool) -> f64 {
        let hits = self
            .places
            .iter()
            .filter(|p| if in_kind { p.1 } else { p.0 } <= n)
            .count();
        hits as f64 / self.places.len().max(1) as f64
    }

    /// The mean of one over the place among every model.
    pub fn mrr(&self) -> f64 {
        let sum: f64 = self.places.iter().map(|p| 1.0 / f64::from(p.0)).sum();
        sum / self.places.len().max(1) as f64
    }

    /// A row under [`HEADER`].
    pub fn row(&self) -> String {
        let mut out = format!("{:<44} {:>6}", self.name, self.len());
        for n in [1, 10, 20] {
            let _ = write!(out, " {:>6.3}", self.hit(n, false));
        }
        let _ = write!(out, " {:>5.3} |", self.mrr());
        for (n, width) in [(1, 16), (5, 8), (20, 16)] {
            let _ = write!(out, " {:>width$.3}", self.hit(n, true));
        }
        out
    }
}
