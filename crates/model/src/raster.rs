use crate::{ALPHA_KEY_REF, Coverage, M2PortraitCamera, RenderSubmesh};

pub(crate) struct Grid {
    cells: Vec<bool>,
    cell_w: f32,
    cell_h: f32,
    x_lo: f32,
    y_lo: f32,
}

pub(crate) const CELLS: usize = 2048;
const _: () = assert!(CELLS.is_multiple_of(2));
pub(crate) const X_SPAN: f32 = 2.2;
const Y_SPAN: f32 = 2.7;

impl Grid {
    pub(crate) fn new(authored_half_height: f32) -> Self {
        let x_lo = -X_SPAN * authored_half_height;
        let y_lo = -Y_SPAN * authored_half_height;
        Self {
            cells: vec![false; CELLS * CELLS],
            cell_w: (2.0 * X_SPAN * authored_half_height) / CELLS as f32,
            cell_h: (2.0 * Y_SPAN * authored_half_height) / CELLS as f32,
            x_lo,
            y_lo,
        }
    }

    pub(crate) fn paint_batch(
        &mut self,
        sub: &RenderSubmesh,
        frame: &EyeFrame,
        near: f32,
        cov: &Coverage,
    ) {
        for tri in sub.indices.as_chunks::<3>().0 {
            let Some(eye) = tri
                .iter()
                .map(|&i| {
                    let p = sub.positions.get(i as usize)?;
                    let uv = sub.uvs.get(i as usize).copied().unwrap_or([0.0; 2]);
                    Some((frame.to_eye(*p), uv))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            for piece in clip_near(&eye, near) {
                let proj: [Vert; 3] = piece.map(|((x, y, z), uv)| Vert {
                    x: x / z,
                    y: y / z,
                    inv_z: 1.0 / z,
                    u_z: uv[0] / z,
                    v_z: uv[1] / z,
                });
                let area = twice_signed_area(&proj);
                if !sub.two_sided && area <= 0.0 {
                    continue;
                }
                if area == 0.0 {
                    continue;
                }
                self.paint_triangle(&proj, area, sub, cov);
            }
        }
    }

    fn paint_triangle(&mut self, t: &[Vert; 3], area: f32, sub: &RenderSubmesh, cov: &Coverage) {
        let (x_min, x_max) = t
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.x), hi.max(v.x))
            });
        let (y_min, y_max) = t
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.y), hi.max(v.y))
            });
        let col_of = |x: f32| ((x - self.x_lo) / self.cell_w).floor();
        let row_of = |y: f32| ((y - self.y_lo) / self.cell_h).floor();
        let c0 = col_of(x_min).max(0.0) as usize;
        let c1 = (col_of(x_max) as isize).min(CELLS as isize - 1);
        let r0 = row_of(y_min).max(0.0) as usize;
        let r1 = (row_of(y_max) as isize).min(CELLS as isize - 1);
        if c1 < 0 || r1 < 0 || c0 > c1 as usize || r0 > r1 as usize {
            return;
        }
        let inv_area = 1.0 / area;
        for r in r0..=r1 as usize {
            let y = self.y_lo + (r as f32 + 0.5) * self.cell_h;
            for c in c0..=c1 as usize {
                let x = self.x_lo + (c as f32 + 0.5) * self.cell_w;
                let w0 = twice_area(&t[1], &t[2], x, y) * inv_area;
                let w1 = twice_area(&t[2], &t[0], x, y) * inv_area;
                let w2 = twice_area(&t[0], &t[1], x, y) * inv_area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let painted = match cov {
                    Coverage::Full => true,
                    Coverage::Alpha(map) => {
                        let inv_z = w0 * t[0].inv_z + w1 * t[1].inv_z + w2 * t[2].inv_z;
                        let u = (w0 * t[0].u_z + w1 * t[1].u_z + w2 * t[2].u_z) / inv_z;
                        let v = (w0 * t[0].v_z + w1 * t[1].v_z + w2 * t[2].v_z) / inv_z;
                        map.sample(u, v, sub.wrap_x, sub.wrap_y) >= ALPHA_KEY_REF
                    }
                };
                if painted {
                    self.cells[r * CELLS + c] = true;
                }
            }
        }
    }

    pub(crate) fn half_extent(&self, axis: Axis, opening: f32) -> f32 {
        let (lines, cells_per_line, step, lo, line_step) = match axis {
            Axis::X => (CELLS, CELLS, self.cell_w, self.y_lo, self.cell_h),
            Axis::Y => (CELLS, CELLS, self.cell_h, self.x_lo, self.cell_w),
        };
        let mut extent = f32::INFINITY;
        let mut any = false;
        for line in 0..lines {
            let pos = lo + (line as f32 + 0.5) * line_step;
            if pos < -opening || pos > opening {
                continue;
            }
            any = true;
            let at = |i: usize| match axis {
                Axis::X => self.cells[line * CELLS + i],
                Axis::Y => self.cells[i * CELLS + line],
            };
            let first_right = cells_per_line / 2;
            let mut right = 0usize;
            while first_right + right < cells_per_line && at(first_right + right) {
                right += 1;
            }
            let mut left = 0usize;
            while left < first_right && at(first_right - 1 - left) {
                left += 1;
            }
            if left == 0 || right == 0 {
                return 0.0;
            }
            extent = extent.min(left.min(right) as f32 * step - step * 0.5);
        }
        if any && extent.is_finite() {
            extent.max(0.0)
        } else {
            0.0
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Axis {
    X,
    Y,
}

#[derive(Clone, Copy)]
pub(crate) struct Vert {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) inv_z: f32,
    pub(crate) u_z: f32,
    pub(crate) v_z: f32,
}

fn twice_area(a: &Vert, b: &Vert, x: f32, y: f32) -> f32 {
    (b.x - a.x) * (y - a.y) - (b.y - a.y) * (x - a.x)
}

/// Positive when counter-clockwise.
pub(crate) fn twice_signed_area(t: &[Vert; 3]) -> f32 {
    twice_area(&t[0], &t[1], t[2].x, t[2].y)
}

type EyeVert = ((f32, f32, f32), [f32; 2]);

pub(crate) fn clip_near(tri: &[EyeVert], near: f32) -> Vec<[EyeVert; 3]> {
    let inside = |p: &EyeVert| p.0.2 >= near;
    let mut poly: Vec<EyeVert> = Vec::with_capacity(4);
    for i in 0..3 {
        let a = tri[i];
        let b = tri[(i + 1) % 3];
        let (ia, ib) = (inside(&a), inside(&b));
        if ia {
            poly.push(a);
        }
        if ia != ib {
            let t = (near - a.0.2) / (b.0.2 - a.0.2);
            let lerp = |p: f32, q: f32| p + (q - p) * t;
            poly.push((
                (lerp(a.0.0, b.0.0), lerp(a.0.1, b.0.1), near),
                [lerp(a.1[0], b.1[0]), lerp(a.1[1], b.1[1])],
            ));
        }
    }
    match poly.len() {
        3 => vec![[poly[0], poly[1], poly[2]]],
        4 => vec![[poly[0], poly[1], poly[2]], [poly[0], poly[2], poly[3]]],
        _ => Vec::new(),
    }
}

pub(crate) struct EyeFrame {
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
}

impl EyeFrame {
    pub(crate) fn of(cam: &M2PortraitCamera) -> Option<Self> {
        let forward = normalize(sub(cam.target, cam.position))?;
        let up0 = rotate_about(WOW_UP, forward, cam.roll);
        let right = normalize(cross(forward, up0))?;
        let up = cross(right, forward);
        Some(Self {
            eye: cam.position,
            right,
            up,
            forward,
        })
    }

    pub(crate) fn to_eye(&self, p: [f32; 3]) -> (f32, f32, f32) {
        let d = sub(p, self.eye);
        (dot(d, self.right), dot(d, self.up), dot(d, self.forward))
    }
}

const WOW_UP: [f32; 3] = [0.0, 0.0, 1.0];

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    (len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
}

fn rotate_about(v: [f32; 3], unit_axis: [f32; 3], angle: f32) -> [f32; 3] {
    if angle == 0.0 {
        return v;
    }
    let (s, c) = angle.sin_cos();
    let k = cross(unit_axis, v);
    let d = dot(unit_axis, v);
    [
        v[0] * c + k[0] * s + unit_axis[0] * d * (1.0 - c),
        v[1] * c + k[1] * s + unit_axis[1] * d * (1.0 - c),
        v[2] * c + k[2] * s + unit_axis[2] * d * (1.0 - c),
    ]
}
