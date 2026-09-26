use crate::text::two_places;

/// A brush's or an area's outline, in zone yards.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Circle {
        c: [f64; 2],
        r: f64,
    },
    /// A band `width` wide along a polyline; each point may carry a height for `flatten`.
    Line {
        pts: Vec<[f64; 2]>,
        z: Vec<Option<f64>>,
        width: f64,
    },
    /// An axis-aligned rectangle between two corners.
    Rect {
        a: [f64; 2],
        b: [f64; 2],
    },
    /// A closed polygon, even-odd.
    Poly {
        pts: Vec<[f64; 2]>,
    },
}

/// Where a point lies in a shape: `t` is 0 at its core and 1 at its rim (everywhere 0 inside a
/// rectangle or polygon), and on a line, the nearest segment and how far along it.
#[derive(Clone, Copy, Debug)]
pub struct Reach {
    pub t: f64,
    pub seg: usize,
    pub along: f64,
}

const CORE: Reach = Reach {
    t: 0.0,
    seg: 0,
    along: 0.0,
};

impl Shape {
    /// The box `[min, max]` of everything the shape reaches.
    pub fn bounds(&self) -> [[f64; 2]; 2] {
        let around = |pts: &[[f64; 2]], pad: f64| {
            let mut lo = [f64::MAX; 2];
            let mut hi = [f64::MIN; 2];
            for p in pts {
                for k in 0..2 {
                    lo[k] = lo[k].min(p[k] - pad);
                    hi[k] = hi[k].max(p[k] + pad);
                }
            }
            [lo, hi]
        };
        match self {
            Shape::Circle { c, r } => [[c[0] - r, c[1] - r], [c[0] + r, c[1] + r]],
            Shape::Line { pts, width, .. } => around(pts, width / 2.0),
            Shape::Rect { a, b } => [
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
            ],
            Shape::Poly { pts } => around(pts, 0.0),
        }
    }

    pub fn reach(&self, p: [f64; 2]) -> Option<Reach> {
        match self {
            Shape::Circle { c, r } => {
                let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
                (d <= *r).then_some(Reach {
                    t: if *r > 0.0 { d / r } else { 0.0 },
                    ..CORE
                })
            }
            Shape::Line { pts, width, .. } => {
                let half = width / 2.0;
                let n = nearest_on_line(pts, p)?;
                (n.distance <= half).then_some(Reach {
                    t: if half > 0.0 { n.distance / half } else { 0.0 },
                    seg: n.seg,
                    along: n.along,
                })
            }
            Shape::Rect { .. } => {
                let [lo, hi] = self.bounds();
                (p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]).then_some(CORE)
            }
            Shape::Poly { pts } => inside(pts, p).then_some(CORE),
        }
    }

    /// Square yards; a line's band counts as its length times its width.
    pub fn area(&self) -> f64 {
        match self {
            Shape::Circle { r, .. } => std::f64::consts::PI * r * r,
            Shape::Line { pts, width, .. } => {
                let len: f64 = pts
                    .windows(2)
                    .map(|w| ((w[1][0] - w[0][0]).powi(2) + (w[1][1] - w[0][1]).powi(2)).sqrt())
                    .sum();
                len * width
            }
            Shape::Rect { a, b } => ((b[0] - a[0]) * (b[1] - a[1])).abs(),
            Shape::Poly { pts } => {
                let mut s = 0.0;
                for i in 0..pts.len() {
                    let (p, q) = (pts[i], pts[(i + 1) % pts.len()]);
                    s += p[0] * q[1] - q[0] * p[1];
                }
                s.abs() / 2.0
            }
        }
    }

    /// The outline as `water.txt` keeps it: `circle X,Y R`, `line X,Y[,Z] ... width W`,
    /// `rect X,Y X,Y` or `poly X,Y X,Y X,Y ...`.
    pub fn describe(&self) -> String {
        let pt = |p: &[f64; 2]| format!("{},{}", two_places(p[0]), two_places(p[1]));
        match self {
            Shape::Circle { c, r } => format!("circle {} {}", pt(c), two_places(*r)),
            Shape::Line { pts, z, width } => {
                let points: Vec<String> = pts
                    .iter()
                    .zip(z)
                    .map(|(p, z)| match z {
                        Some(z) => format!("{},{}", pt(p), two_places(*z)),
                        None => pt(p),
                    })
                    .collect();
                format!("line {} width {}", points.join(" "), two_places(*width))
            }
            Shape::Rect { a, b } => format!("rect {} {}", pt(a), pt(b)),
            Shape::Poly { pts } => {
                let points: Vec<String> = pts.iter().map(pt).collect();
                format!("poly {}", points.join(" "))
            }
        }
    }
}

struct Nearest {
    distance: f64,
    seg: usize,
    along: f64,
}

fn nearest_on_line(pts: &[[f64; 2]], p: [f64; 2]) -> Option<Nearest> {
    let mut best: Option<Nearest> = None;
    if let [only] = pts {
        let distance = ((p[0] - only[0]).powi(2) + (p[1] - only[1]).powi(2)).sqrt();
        best = Some(Nearest {
            distance,
            seg: 0,
            along: 0.0,
        });
    }
    for s in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[s], pts[s + 1]);
        let ab = [b[0] - a[0], b[1] - a[1]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1];
        let u = if len2 > 0.0 {
            (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + u * ab[0], a[1] + u * ab[1]];
        let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
        if best.as_ref().is_none_or(|b| d < b.distance) {
            best = Some(Nearest {
                distance: d,
                seg: s,
                along: u,
            });
        }
    }
    best
}

fn inside(pts: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut odd = false;
    let n = pts.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (pts[i], pts[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            odd = !odd;
        }
        j = i;
    }
    odd
}

pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A brush's strength at `t` (0 core, 1 rim): full out to `1 − falloff`, then easing to 0 at the
/// rim. A falloff of 0 is a hard edge, and 1 eases from the middle.
pub fn falloff(t: f64, falloff: f64) -> f64 {
    let f = falloff.clamp(0.0, 1.0);
    if t <= 1.0 - f {
        1.0
    } else if f == 0.0 {
        0.0
    } else {
        smoothstep((1.0 - t) / f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_reach_as_drawn() {
        let c = Shape::Circle {
            c: [10.0, 10.0],
            r: 5.0,
        };
        assert!((c.reach([13.0, 14.0]).map_or(9.0, |r| r.t) - 1.0).abs() < 1e-12);
        assert!(c.reach([16.0, 10.0]).is_none());
        let l = Shape::Line {
            pts: vec![[0.0, 0.0], [10.0, 0.0]],
            z: vec![None, None],
            width: 4.0,
        };
        let r = l.reach([5.0, 1.0]).expect("inside the band");
        assert!((r.t - 0.5).abs() < 1e-12 && (r.along - 0.5).abs() < 1e-12);
        assert!(l.reach([5.0, 2.5]).is_none());
        let p = Shape::Poly {
            pts: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
        };
        assert!(p.reach([2.0, 2.0]).is_some() && p.reach([8.0, 8.0]).is_none());
        assert!((p.area() - 50.0).abs() < 1e-9);
        assert!((falloff(0.2, 0.5) - 1.0).abs() < f64::EPSILON);
        assert!(falloff(1.0, 0.5).abs() < f64::EPSILON);
        assert!((falloff(0.99, 0.0) - 1.0).abs() < f64::EPSILON);
    }
}
