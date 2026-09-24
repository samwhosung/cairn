/// One segment's flipbook ramp from cell `begin` to cell `end`; a decreasing pair plays it
/// backwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRamp {
    pub begin: u16,
    pub end: u16,
    base: i32,
    span: i32,
}

impl CellRamp {
    pub fn new(begin: u16, end: u16) -> Self {
        let (b, e) = (i32::from(begin), i32::from(end));
        let (base, span) = if e >= b {
            (b, e - b + 1)
        } else {
            (b + 1, e - b - 1)
        };
        Self {
            begin,
            end,
            base,
            span,
        }
    }

    /// The cell at segment fraction `t`, already inset by [`OverLife::sample`]: the floor's low
    /// byte, as the client keeps it, not bounded to the atlas.
    pub fn sample(&self, t: f32) -> u16 {
        let v = self.base as f32 + self.span as f32 * t;
        ((v.floor() as i32) & 0xFF) as u16
    }
}

/// What a particle looks like over its life, by its age over its lifespan `u`: colour and size
/// are three keys lerped over two segments split at [`Self::mid`], and the head quad and the
/// tail streak each walk a flipbook ramp per segment.
#[derive(Debug, Clone, Copy)]
pub struct OverLife {
    /// Where segment A ends; `u == mid` is still A.
    pub mid: f32,
    /// RGBA keys.
    pub color: [[f32; 4]; 3],
    /// Half-extent keys.
    pub scale: [f32; 3],
    pub head_cells: [CellRamp; 2],
    pub tail_cells: [CellRamp; 2],
    /// How many times each segment's flipbook cycles; colour and size do not.
    pub repeat: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct OverLifeSample {
    pub color: [f32; 4],
    /// Half-extent.
    pub size: f32,
    pub head_cell: u16,
    pub tail_cell: u16,
}

impl OverLife {
    /// Every ramp at `u`, clamped to `0..=1`. The client insets the segment fraction to
    /// `0.005..=0.995` for colour, size and cells alike.
    #[allow(
        clippy::float_cmp,
        reason = "the client tests the count against exactly 1"
    )]
    pub fn sample(&self, u: f32) -> OverLifeSample {
        let u = u.clamp(0.0, 1.0);
        let mid = self.mid.clamp(1e-3, 1.0);
        let (k0, k1, t, seg) = if u <= mid {
            (0, 1, u / mid, 0)
        } else {
            (1, 2, (u - mid) / (1.0 - mid).max(1e-3), 1)
        };
        let t = t.clamp(0.0, 1.0) * 0.99 + 0.005;
        let mut color = [0.0; 4];
        for (c, slot) in color.iter_mut().enumerate() {
            *slot = self.color[k0][c] + (self.color[k1][c] - self.color[k0][c]) * t;
        }
        let size = self.scale[k0] + (self.scale[k1] - self.scale[k0]) * t;
        let ct = if self.repeat[seg] == 1.0 {
            t
        } else {
            (t * self.repeat[seg]).fract()
        };
        OverLifeSample {
            color,
            size,
            head_cell: self.head_cells[seg].sample(ct),
            tail_cell: self.tail_cells[seg].sample(ct),
        }
    }
}

/// Chords a segment's arc length is measured over.
const CHORDS: usize = 16;

/// A spline emitter's curve: a chain of cubic Bézier segments, `3K + 1` control points
/// (`point, out tangent, in tangent, point, …`), walked by arc length.
#[derive(Debug, Clone)]
pub struct SplineData {
    /// Offsets from the emitter's position.
    pub points: Vec<[f32; 3]>,
    /// The arc fraction at each segment boundary, `0` to `1`.
    knots: Vec<f32>,
}

impl SplineData {
    /// `None` below one segment or off the `3K + 1` count.
    pub fn new(points: Vec<[f32; 3]>) -> Option<Self> {
        let k = points.len().checked_sub(1)? / 3;
        if k == 0 || points.len() != 3 * k + 1 {
            return None;
        }
        let mut knots = vec![0.0f32];
        for seg in 0..k {
            let mut len = 0.0;
            let mut prev = Self::bezier(&points[3 * seg..3 * seg + 4], 0.0);
            for i in 1..=CHORDS {
                let p = Self::bezier(&points[3 * seg..3 * seg + 4], i as f32 / CHORDS as f32);
                len += ((p[0] - prev[0]).powi(2)
                    + (p[1] - prev[1]).powi(2)
                    + (p[2] - prev[2]).powi(2))
                .sqrt();
                prev = p;
            }
            knots.push(knots[seg] + len);
        }
        let total = knots[k];
        if total > 0.0 {
            for kn in &mut knots {
                *kn /= total;
            }
        }
        Some(Self { points, knots })
    }

    fn bezier(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            (1.0 - u).powi(3),
            3.0 * u * (1.0 - u).powi(2),
            3.0 * u * u * (1.0 - u),
            u.powi(3),
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    fn bezier_deriv(p: &[[f32; 3]], u: f32) -> [f32; 3] {
        let w = [
            -3.0 * (1.0 - u).powi(2),
            3.0 * (1.0 - u) * (1.0 - 3.0 * u),
            3.0 * u * (2.0 - 3.0 * u),
            3.0 * u * u,
        ];
        std::array::from_fn(|c| (0..4).map(|i| w[i] * p[i][c]).sum())
    }

    fn locate(&self, t: f32) -> (usize, f32) {
        let k = self.knots.len() - 1;
        let seg = self.knots[1..k]
            .iter()
            .position(|&kn| t < kn)
            .unwrap_or(k - 1);
        let (a, b) = (self.knots[seg], self.knots[seg + 1]);
        (seg, ((t - a) / (b - a).max(1e-6)).clamp(0.0, 1.0))
    }

    /// The point at arc fraction `t`; the first or last point outside `0..1`.
    pub fn eval(&self, t: f32) -> [f32; 3] {
        if t <= 0.0 {
            return self.points[0];
        }
        if t >= 1.0 {
            return self.points[self.points.len() - 1];
        }
        let (seg, u) = self.locate(t);
        Self::bezier(&self.points[3 * seg..3 * seg + 4], u)
    }

    /// The curve's derivative at arc fraction `t`, not normalized.
    pub fn tangent(&self, t: f32) -> [f32; 3] {
        let (seg, u) = self.locate(t.clamp(0.0, 1.0));
        Self::bezier_deriv(&self.points[3 * seg..3 * seg + 4], u)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_spline_is_walked_by_arc_length() {
        let x = |v: f32| [v, 0.0, 0.0];
        let s = SplineData::new(vec![
            x(0.0),
            x(1.0 / 3.0),
            x(2.0 / 3.0),
            x(1.0),
            x(2.0),
            x(3.0),
            x(4.0),
        ])
        .expect("3K+1 points");
        assert!(
            (s.eval(0.25)[0] - 1.0).abs() < 1e-4,
            "the joint is a quarter along"
        );
        assert!((s.eval(0.625)[0] - 2.5).abs() < 1e-4);
        assert_eq!(s.eval(-0.5), [0.0, 0.0, 0.0]);
        assert_eq!(s.eval(1.5), [4.0, 0.0, 0.0]);
        let tan = s.tangent(0.1);
        assert!(tan[0] > 0.0 && tan[1] == 0.0 && tan[2] == 0.0);
        assert!(SplineData::new(vec![x(0.0)]).is_none());
    }

    #[test]
    fn a_cell_ramp_lands_on_its_ends_in_both_directions() {
        let at = |begin: u16, end: u16| {
            let r = CellRamp::new(begin, end);
            [0.0_f32, 0.25, 0.5, 0.75, 1.0]
                .map(|t| r.sample(t * 0.99 + 0.005))
                .to_vec()
        };
        assert_eq!(at(0, 15), vec![0, 4, 8, 11, 15]);
        assert_eq!(at(6, 5), vec![6, 6, 6, 5, 5]);
        assert_eq!(at(31, 16), vec![31, 27, 24, 20, 16]);
        assert_eq!(at(15, 0), vec![15, 11, 8, 4, 0]);
        for (b, e) in [(0, 15), (8, 16), (31, 16), (15, 0), (6, 5), (7, 7), (0, 63)] {
            let r = CellRamp::new(b, e);
            assert_eq!(r.sample(0.005), b);
            assert_eq!(r.sample(0.995), e);
        }
        let ol = OverLife {
            mid: 0.5,
            color: [[1.0; 4]; 3],
            scale: [1.0; 3],
            head_cells: [CellRamp::new(0, 7), CellRamp::new(31, 16)],
            tail_cells: [CellRamp::new(3, 3), CellRamp::new(9, 4)],
            repeat: [1.0; 2],
        };
        assert_eq!(ol.sample(0.0).head_cell, 0);
        assert_eq!(ol.sample(1.0).head_cell, 16);
        assert_eq!(ol.sample(0.0).tail_cell, 3);
        assert_eq!(ol.sample(1.0).tail_cell, 4);
        assert_eq!(ol.sample(0.5).head_cell, 7, "mid is still segment A");
    }

    #[test]
    fn colour_and_size_take_the_same_inset() {
        let ol = OverLife {
            mid: 0.5,
            color: [[0.0; 4], [1.0; 4], [1.0; 4]],
            scale: [0.0, 100.0, 100.0],
            head_cells: [CellRamp::new(0, 0); 2],
            tail_cells: [CellRamp::new(0, 0); 2],
            repeat: [1.0; 2],
        };
        let start = ol.sample(0.0);
        assert!((start.size - 0.5).abs() < 1e-4);
        assert!((start.color[0] - 0.005).abs() < 1e-4);
        assert!((ol.sample(0.5).size - 99.5).abs() < 1e-3);
    }
}
