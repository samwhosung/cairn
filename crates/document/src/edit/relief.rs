use crate::choice::{hash, hash_ignoring_case, key};
use crate::command::Relief;
use crate::image::Image;
use crate::relief::{OVERLAP, PATCH, Source};
use crate::shape::falloff;
use crate::text::two_places;
use crate::zone::{Heights, Zone};

use super::ground::{Vertex, get, reached};

const QUILT_STRIDE: usize = PATCH - OVERLAP;
const PATCHES_TRIED: u64 = 16;
const ROUGHNESS_BANDS: usize = 16;
const ROUGHNESS_DRAW: u64 = u64::MAX;

struct Quilt {
    north_west: (usize, usize),
    relief: Heights,
    laid: Vec<bool>,
}

struct Laid {
    west: bool,
    north: bool,
}

struct PatchVertex {
    quilt_index: usize,
    source_relief: f32,
    cells_into_patch: [f64; 2],
}

impl Quilt {
    fn lay(source: &Source, key: u64, (a0, a1): (usize, usize), (b0, b1): (usize, usize)) -> Quilt {
        let cols = (a1 - a0) * QUILT_STRIDE + PATCH;
        let rows = (b1 - b0) * QUILT_STRIDE + PATCH;
        let relief = Heights::flat(cols, rows, 0.0);
        let mut q = Quilt {
            north_west: (a0 * QUILT_STRIDE, b0 * QUILT_STRIDE),
            laid: vec![false; relief.len()],
            relief,
        };
        for b in b0..=b1 {
            for a in a0..=a1 {
                let here = ((a - a0) * QUILT_STRIDE, (b - b0) * QUILT_STRIDE);
                let from = q.pick(source, |k| hash(&[key, a as u64, b as u64, k]), here);
                let laid = Laid {
                    west: a > a0,
                    north: b > b0,
                };
                q.paste(source, from, here, &laid);
            }
        }
        q
    }

    fn pick(&self, source: &Source, h: impl Fn(u64) -> u64, here: (usize, usize)) -> (u32, u32) {
        let band = drawn_roughness_band(source, h(ROUGHNESS_DRAW));
        let width = band.len() as u64;
        (0..PATCHES_TRIED)
            .map(|k| {
                let from = source.patches[band.start + (h(k) % width) as usize];
                (self.misfit(source, from, here), k, from)
            })
            .min_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)))
            .map_or(source.patches[band.start], |(_, _, from)| from)
    }

    fn patch(
        &self,
        source: &Source,
        (fx, fy): (u32, u32),
        here: (usize, usize),
    ) -> Vec<PatchVertex> {
        let (fx, fy) = (fx as usize, fy as usize);
        let (ours, theirs) = (&self.relief, &source.relief);
        let mut out = Vec::with_capacity((PATCH + 1) * (PATCH + 1) + PATCH * PATCH);
        for y in 0..=PATCH {
            for x in 0..=PATCH {
                out.push(PatchVertex {
                    quilt_index: (here.1 + y) * (ours.cols + 1) + here.0 + x,
                    source_relief: theirs.outer(fx + x, fy + y),
                    cells_into_patch: [x as f64, y as f64],
                });
            }
        }
        for y in 0..PATCH {
            for x in 0..PATCH {
                out.push(PatchVertex {
                    quilt_index: ours.outer.len() + (here.1 + y) * ours.cols + here.0 + x,
                    source_relief: theirs.inner(fx + x, fy + y),
                    cells_into_patch: [x as f64 + 0.5, y as f64 + 0.5],
                });
            }
        }
        out
    }

    fn misfit(&self, source: &Source, from: (u32, u32), here: (usize, usize)) -> f64 {
        self.patch(source, from, here)
            .iter()
            .filter(|v| self.laid[v.quilt_index])
            .map(|v| f64::from(self.relief.get(v.quilt_index) - v.source_relief).powi(2))
            .sum()
    }

    fn paste(&mut self, source: &Source, from: (u32, u32), here: (usize, usize), laid: &Laid) {
        let ease = |t: f64, after: bool| {
            if after {
                (t / OVERLAP as f64).clamp(0.0, 1.0)
            } else {
                1.0
            }
        };
        for v in self.patch(source, from, here) {
            let [x, y] = v.cells_into_patch;
            let f = ease(x, laid.west).min(ease(y, laid.north)) as f32;
            let k = v.quilt_index;
            let value = if self.laid[k] {
                self.relief.get(k) * (1.0 - f) + v.source_relief * f
            } else {
                v.source_relief
            };
            self.relief.set(k, value);
            self.laid[k] = true;
        }
    }

    fn relief_at(&self, v: Vertex) -> f32 {
        let (x, y) = (v.i - self.north_west.0, v.j - self.north_west.1);
        if v.inner {
            self.relief.inner(x, y)
        } else {
            self.relief.outer(x, y)
        }
    }
}

/// The source's patches, smoothest first, as rough as the one `draw` picks at random.
fn drawn_roughness_band(source: &Source, draw: u64) -> std::ops::Range<usize> {
    let n = source.patches.len();
    let width = (n / ROUGHNESS_BANDS).max(1);
    let drawn_rank = (draw % n as u64) as usize;
    let first = drawn_rank.saturating_sub(width / 2).min(n - width);
    first..first + width
}

pub fn apply(z: &mut Zone, r: &Relief, source: &Source) -> Result<(String, Image), String> {
    let hits = reached(z, &r.area);
    if hits.is_empty() {
        return Err("the area reaches no vertex of the zone".into());
    }
    let (mut lo, mut hi) = ([usize::MAX; 2], [0usize; 2]);
    for (v, _) in &hits {
        lo = [lo[0].min(v.i), lo[1].min(v.j)];
        hi = [hi[0].max(v.i), hi[1].max(v.j)];
    }
    let key = key(hash(&[r.seed, hash_ignoring_case(&source.zone)]));
    let quilt = Quilt::lay(
        source,
        key,
        (lo[0] / QUILT_STRIDE, hi[0] / QUILT_STRIDE),
        (lo[1] / QUILT_STRIDE, hi[1] / QUILT_STRIDE),
    );
    let before: Vec<(usize, f32)> = hits.iter().map(|&(v, _)| (v.index(z), get(z, v))).collect();
    let (mut low, mut high) = (f64::MAX, f64::MIN);
    for &(v, reach) in &hits {
        let w = falloff(reach.t, r.falloff) * r.strength;
        let h = f64::from(get(z, v)) + w * f64::from(quilt.relief_at(v));
        z.heights.set(v.index(z), h as f32);
        low = low.min(h);
        high = high.max(h);
    }
    let mut changed: Vec<(usize, f32)> = before
        .into_iter()
        .filter(|&(k, was)| z.heights.get(k).to_bits() != was.to_bits())
        .collect();
    changed.sort_unstable_by_key(|&(k, _)| k);
    let reply = format!(
        "{} vertices reached, {} moved by the relief of {} (it spreads {} yd about its own \
         smoothed shape); the ground there now {}..{} yd",
        hits.len(),
        changed.len(),
        source.zone,
        two_places(source.spread * r.strength),
        two_places(low),
        two_places(high)
    );
    Ok((
        reply,
        Image {
            heights: Image::heights_of(changed),
            ..Image::default()
        },
    ))
}
