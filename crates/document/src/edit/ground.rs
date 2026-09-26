use crate::choice::{key, noise};
use crate::command::{Ground, GroundOp};
use crate::frame::CELL;
use crate::image::{HeightRun, Image};
use crate::shape::{Reach, Shape, falloff};
use crate::text::two_places;
use crate::zone::Zone;

#[derive(Clone, Copy, Debug)]
struct Vertex {
    inner: bool,
    i: usize,
    j: usize,
}

impl Vertex {
    fn pos(self) -> [f64; 2] {
        let o = if self.inner { 0.5 } else { 0.0 };
        [(self.i as f64 + o) * CELL, (self.j as f64 + o) * CELL]
    }

    fn index(self, z: &Zone) -> usize {
        let h = &z.heights;
        if self.inner {
            h.outer.len() + self.j * h.cols + self.i
        } else {
            self.j * (h.cols + 1) + self.i
        }
    }
}

fn reached(z: &Zone, s: &Shape) -> Vec<(Vertex, Reach)> {
    let (cols, rows) = (z.heights.cols, z.heights.rows);
    let [lo, hi] = s.bounds();
    let range = |a: f64, b: f64, n: usize, o: f64| {
        let a = (a / CELL - o).floor().max(0.0) as usize;
        let b = ((b / CELL - o).ceil().max(0.0) as usize).min(n);
        a..=b.max(a)
    };
    let mut out = Vec::new();
    for inner in [false, true] {
        let (o, nc, nr) = if inner {
            (0.5, cols - 1, rows - 1)
        } else {
            (0.0, cols, rows)
        };
        for j in range(lo[1], hi[1], nr, o).filter(|&j| j <= nr) {
            for i in range(lo[0], hi[0], nc, o).filter(|&i| i <= nc) {
                let v = Vertex { inner, i, j };
                if let Some(r) = s.reach(v.pos()) {
                    out.push((v, r));
                }
            }
        }
    }
    out
}

fn get(z: &Zone, v: Vertex) -> f32 {
    if v.inner {
        z.heights.inner(v.i, v.j)
    } else {
        z.heights.outer(v.i, v.j)
    }
}

/// The four nearest vertices of the other lattice: a cell's corners for its centre, the middles of
/// the four cells around a corner for the corner, and the vertex itself for any off the zone.
fn neighbours(z: &Zone, v: Vertex) -> [f32; 4] {
    let h = &z.heights;
    [(0usize, 0usize), (1, 0), (0, 1), (1, 1)].map(|(di, dj)| {
        if v.inner {
            return h.outer(v.i + di, v.j + dj);
        }
        match (
            (v.i + di).checked_sub(1).filter(|&i| i < h.cols),
            (v.j + dj).checked_sub(1).filter(|&j| j < h.rows),
        ) {
            (Some(i), Some(j)) => h.inner(i, j),
            _ => h.outer(v.i, v.j),
        }
    })
}

fn neighbour_mean(z: &Zone, v: Vertex) -> f64 {
    let mut s = 0.0;
    for n in neighbours(z, v) {
        s += f64::from(n);
    }
    s / 4.0
}

/// The height a brush of weight `w` gives a vertex, from the heights before its pass.
fn new_height(
    z: &Zone,
    v: Vertex,
    r: Reach,
    w: f64,
    op: &GroundOp,
    brush: &Shape,
    line_z: &[f64],
) -> Option<f64> {
    let h = f64::from(get(z, v));
    let p = v.pos();
    Some(match *op {
        GroundOp::Raise(a) => h + a * w,
        GroundOp::Lower(a) => h + -a * w,
        GroundOp::Flatten { to, strength } => {
            let target = match (to, brush) {
                (Some(t), _) => t,
                (None, Shape::Line { .. }) => {
                    let a = line_z[r.seg];
                    let b = line_z.get(r.seg + 1).copied().unwrap_or(a);
                    a + (b - a) * r.along
                }
                (None, _) => return None,
            };
            h + (target - h) * w * strength
        }
        GroundOp::Smooth { strength, .. } => h + (neighbour_mean(z, v) - h) * w * strength,
        GroundOp::Roughen {
            amplitude,
            feature_size,
            seed,
        } => h + amplitude * w * noise(seed, [p[0] / feature_size, p[1] / feature_size]),
    })
}

pub fn apply(z: &mut Zone, g: &Ground) -> Result<(String, Image), String> {
    let op = match &g.op {
        GroundOp::Roughen {
            amplitude,
            feature_size,
            seed,
        } => GroundOp::Roughen {
            amplitude: *amplitude,
            feature_size: *feature_size,
            seed: key(*seed),
        },
        o => o.clone(),
    };
    let s = &g.brush;
    let hits = reached(z, s);
    if hits.is_empty() {
        return Err("the brush reaches no vertex of the zone".into());
    }
    let before: Vec<(usize, f32)> = hits.iter().map(|&(v, _)| (v.index(z), get(z, v))).collect();
    let line_z: Vec<f64> = match s {
        Shape::Line { pts, z: zs, .. } => pts
            .iter()
            .zip(zs)
            .map(|(p, h)| h.unwrap_or_else(|| z.heights.ground(*p)))
            .collect(),
        _ => Vec::new(),
    };
    let passes = match op {
        GroundOp::Smooth { passes, .. } => passes.max(1),
        _ => 1,
    };
    let mut moved = 0usize;
    for _ in 0..passes {
        let new: Vec<(Vertex, f32)> = hits
            .iter()
            .filter_map(|&(v, r)| {
                let w = falloff(r.t, g.falloff);
                (w > 0.0)
                    .then(|| new_height(z, v, r, w, &op, s, &line_z))
                    .flatten()
                    .map(|n| (v, n as f32))
            })
            .collect();
        for (v, n) in new {
            if n.to_bits() != get(z, v).to_bits() {
                moved += 1;
            }
            z.heights.set(v.index(z), n);
        }
    }
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for &(v, _) in &hits {
        let h = f64::from(get(z, v));
        lo = lo.min(h);
        hi = hi.max(h);
    }
    let mut changed: Vec<(usize, f32)> = before
        .into_iter()
        .filter(|&(k, was)| z.heights.get(k).to_bits() != was.to_bits())
        .collect();
    changed.sort_unstable_by_key(|&(k, _)| k);
    let heights: Vec<HeightRun> = Image::heights_of(changed);
    let reply = format!(
        "{} vertices reached, {moved} moved; the ground there now {}..{} yd",
        hits.len(),
        two_places(lo),
        two_places(hi)
    );
    Ok((
        reply,
        Image {
            heights,
            ..Image::default()
        },
    ))
}
