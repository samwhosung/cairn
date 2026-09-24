//! The client's cloud field. The arithmetic follows the client's own rounding step by step, so the
//! bytes come out the same.
//!
//! One departure: where the client's arc cosine of a sky direction would go out of range, this one
//! clamps, which reads the zenith's cell. The lighting body's cell goes out of range whenever the
//! body is up, so a risen body lights the clouds' glow from overhead.

use bevy::math::Vec3;

use super::tables::{COVERAGE_CURVE, PERM, fade_table, gradient_table};

pub(super) const SIDE: usize = 128;
const ROWS_PER_TICK: usize = 32;
const OCTAVES: usize = 4;
const SLOPE_OCTAVES: usize = 3;
const REGEN_SECS: f32 = 0.1;
const BASE_FREQ: [u16; OCTAVES] = [16, 32, 64, 128];
const INV_255: f32 = 1.0 / 255.0;
const LIGHT_HEIGHT_CELLS: f32 = 64.0;
const TRANSPARENT_WHITE: [u8; 4] = [255, 255, 255, 0];

fn perm(i: u32) -> u32 {
    u32::from(PERM[(i & 0xff) as usize])
}

#[derive(Clone, Copy, Default)]
struct LatticeKey(u16);

impl LatticeKey {
    fn cell(self) -> u32 {
        u32::from(self.0 >> 8)
    }

    fn fade_index(self) -> usize {
        usize::from(self.0 & 0xff)
    }

    fn step(&mut self, by: u16) {
        self.0 = self.0.wrapping_add(by);
    }
}

#[derive(Default)]
struct Octave {
    freq: u16,
    row_key: LatticeKey,
    col_key: LatticeKey,
    amp: f32,
    x0: u32,
    x1: u32,
    y0: u32,
    y1: u32,
    corners: [Corner; 4],
    cached: u32,
}

#[derive(Clone, Copy, Default)]
struct Corner {
    at: f32,
    step: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct CloudFrame {
    pub glow: [f32; 3],
    pub slope: [f32; 3],
    pub base: [f32; 3],
    pub glow_dir: Vec3,
    pub glow_track: f32,
}

pub(super) struct CloudKernel {
    tile: Vec<u8>,
    accum: Vec<f32>,
    slopes: Vec<[f32; 2]>,
    prevrow: Vec<f32>,
    rgba: Vec<[u8; 4]>,
    scroll: usize,
    pass: LatticeKey,
    countdown: f32,
    gradient: [f32; 256],
    fade: [f32; 256],
}

impl Default for CloudKernel {
    fn default() -> Self {
        CloudKernel {
            tile: vec![0; SIDE * SIDE],
            accum: vec![0.0; SIDE * SIDE],
            slopes: vec![[0.0; 2]; SIDE * SIDE],
            prevrow: vec![0.0; SIDE],
            rgba: vec![TRANSPARENT_WHITE; SIDE * SIDE],
            scroll: 0,
            pass: LatticeKey(0),
            countdown: 0.0,
            gradient: gradient_table(),
            fade: fade_table(),
        }
    }
}

impl CloudKernel {
    pub(super) fn regen_if_due(&mut self, dt: f32, density: f32, frame: &CloudFrame) -> bool {
        self.countdown -= dt;
        if self.countdown > 0.0 {
            return false;
        }
        self.countdown = REGEN_SECS;
        self.regen(density, ROWS_PER_TICK, frame);
        true
    }

    pub(super) fn rebuild(&mut self, density: f32, frame: &CloudFrame) {
        self.scroll = 0;
        self.regen(density, SIDE, frame);
        self.countdown = REGEN_SECS;
    }

    pub(super) fn recolor(&mut self, frame: &CloudFrame) {
        self.color_band(0, SIDE, frame);
    }

    fn regen(&mut self, density: f32, rows: usize, frame: &CloudFrame) {
        let threshold = ((1.0 - density.clamp(0.0, 1.0)) * 255.0) as i32;
        let scroll = self.scroll;
        let band = scroll * SIDE..(scroll + rows).min(SIDE) * SIDE;
        self.accumulate(scroll, rows);
        for cell in band {
            let byte = float_byte(f64::from(self.accum[cell]) * 64.0 + 128.0);
            let idx = i32::from(byte) - threshold;
            self.tile[cell] = if idx >= 0 {
                COVERAGE_CURVE[idx as usize]
            } else {
                0
            };
        }
        self.color_band(scroll, rows, frame);
        self.scroll += rows;
        if self.scroll >= SIDE {
            self.pass.step(1);
            self.scroll = 0;
        }
    }

    fn accumulate(&mut self, scroll: usize, rows: usize) {
        let seed = self.pass.cell();
        let (slice_a, slice_b) = (perm(seed), perm(seed + 1));
        let fade_t = f64::from(self.fade[self.pass.fade_index()]);
        let mut oct: [Octave; OCTAVES] = std::array::from_fn(|c| Octave {
            freq: BASE_FREQ[c],
            row_key: LatticeKey((scroll as u32).wrapping_mul(u32::from(BASE_FREQ[c])) as u16),
            amp: 1.0 / (1u32 << c) as f32,
            cached: u32::MAX,
            ..Octave::default()
        });
        for v in &mut self.accum[scroll * SIDE..(scroll + rows).min(SIDE) * SIDE] {
            *v = 0.0;
        }
        let f = f64::from;
        for row in 0..rows {
            let base = (scroll + row) * SIDE;
            for o in &mut oct {
                let bv = o.row_key.cell();
                o.x0 = perm(slice_a + bv);
                o.x1 = perm(slice_a + bv + 1);
                o.y0 = perm(bv + slice_b);
                o.y1 = perm(bv + 1 + slice_b);
                o.col_key = self.pass;
                o.cached = u32::MAX;
            }
            let mut prev_accum = 0.0f32;
            for col in 0..SIDE {
                let cell = base + col;
                for (oi, o) in oct.iter_mut().enumerate() {
                    let fade_row = f(self.fade[o.row_key.fade_index()]);
                    let cz = o.col_key.cell();
                    if cz != o.cached {
                        o.cached = cz;
                        let g = |s: u32| self.gradient[perm(s) as usize];
                        for (k, s) in [o.x0, o.x1, o.y0, o.y1].into_iter().enumerate() {
                            let at = g(s + cz);
                            o.corners[k] = Corner {
                                at,
                                step: g(s + cz + 1) - at,
                            };
                        }
                    }
                    let [c00, c10, c01, c11] = o.corners;
                    let fx = f(self.fade[o.col_key.fade_index()]);
                    let v7 = fx * f(c00.step) + f(c00.at);
                    let v8 = f((fx * f(c01.step) + f(c01.at)) as f32);
                    let v7b = (fx * f(c10.step) + f(c10.at) - v7) * fade_row + v7;
                    let v8b = (fx * f(c11.step) + f(c11.at) - v8) * fade_row + v8;
                    o.col_key.step(o.freq);
                    let stored =
                        (((v8b - v7b) * fade_t + v7b) * f(o.amp) + f(self.accum[cell])) as f32;
                    self.accum[cell] = stored;
                    if oi + 1 == SLOPE_OCTAVES {
                        self.slopes[cell][0] = (f(prev_accum) - f(stored)) as f32;
                        self.slopes[cell][1] = (f(self.prevrow[col]) - f(stored)) as f32;
                        prev_accum = stored;
                        self.prevrow[col] = stored;
                    }
                }
            }
            for o in &mut oct {
                o.row_key.step(o.freq);
            }
        }
    }

    fn color_band(&mut self, start: usize, rows: usize, frame: &CloudFrame) {
        let f = f64::from;
        let body = body_cells(frame.glow_dir);
        for row in start..(start + rows).min(SIDE) {
            for col in 0..SIDE {
                let g = row * SIDE + col;
                let t = self.tile[g];
                if t == 0 {
                    if col != 0 {
                        self.rgba[g] = clear_with_colour_of(self.rgba[g - 1]);
                    }
                    continue;
                }
                let n = u32::from((255u8.wrapping_sub(t) >> 1).wrapping_add(0x40));
                let p = f(INV_255) * f64::from(n);
                let mut ch: [f32; 3] =
                    std::array::from_fn(|i| (f(frame.slope[i]) * p + f(frame.base[i])) as f32);
                if let Some((su, sv)) = body {
                    let vx = (f(su) - f64::from(col as u32)) as f32;
                    let vy = (f(sv) - f(row as f32)) as f32;
                    let vz = LIGHT_HEIGHT_CELLS;
                    let s = self.slopes[g];
                    let len_v_sq = ((f(vz) * f(vz) + f(vy) * f(vy)) + f(vx) * f(vx)) as f32;
                    let len_s_sq = ((f(s[0]) * f(s[0]) + f(s[1]) * f(s[1])) + 1.0) as f32;
                    let dot = f(vx) * f(s[0]) + f(vy) * f(s[1]) + f(vz);
                    let cos_t = dot * (f(fast_inv_sqrt(len_v_sq)) * f(fast_inv_sqrt(len_s_sq)));
                    if cos_t > 0.0 {
                        let m = cos_t * f(frame.glow_track);
                        for (c, glow) in ch.iter_mut().zip(frame.glow) {
                            *c = (f(glow) * m + f(*c)) as f32;
                        }
                    }
                }
                self.rgba[g] = [
                    pack_channel(f(ch[0])),
                    pack_channel(f(ch[1])),
                    pack_channel(f(ch[2])),
                    t,
                ];
            }
        }
    }

    pub(super) fn rgba(&self) -> &[[u8; 4]] {
        &self.rgba
    }
}

fn project_cells(d: Vec3) -> Option<(f32, f32)> {
    let len = f64::from(d.length());
    if len < 1e-6 {
        return None;
    }
    let quarter_pi = f64::from(std::f32::consts::FRAC_PI_4);
    let c = f64::from(d.y) + f64::from(std::f32::consts::FRAC_PI_4.cos());
    let theta = (c / len).clamp(-1.0, 1.0).acos();
    let phase = theta.min(quarter_pi) / quarter_pi * 0.5;
    let hyp = (f64::from(d.x) * f64::from(d.x) + f64::from(d.z) * f64::from(d.z)).sqrt();
    let (cx, cy) = if hyp > 1e-5 {
        let inv = (1.0 / hyp) as f32;
        (
            f64::from(inv) * f64::from(d.x),
            f64::from(inv) * f64::from(d.z),
        )
    } else {
        (0.0, 0.0)
    };
    Some((
        ((cx * phase + 0.5) * SIDE as f64) as f32,
        ((cy * phase + 0.5) * SIDE as f64) as f32,
    ))
}

fn body_cells(dir: Vec3) -> Option<(f32, f32)> {
    let f = f64::from;
    let k = -(f(0.25f32) * f(std::f32::consts::PI)).cos();
    let a = (f(dir.x) * f(dir.x) + f(dir.y) * f(dir.y) + f(dir.z) * f(dir.z)) as f32;
    let nk = -k * f(dir.y);
    let b = (nk + nk) as f32;
    let c = (k * k - 1.0) as f32;
    let t = quadratic_larger_root(a, b, c)?;
    project_cells(Vec3::new(
        (f(t) * f(dir.x)) as f32,
        (f(t) * f(dir.y)) as f32,
        (f(t) * f(dir.z)) as f32,
    ))
}

fn quadratic_larger_root(a: f32, b: f32, c: f32) -> Option<f32> {
    let f = f64::from;
    let g = f(a) * f(c) * 4.0;
    let h = f(b) * f(b);
    if h.is_nan() || g.is_nan() || h <= g {
        return None;
    }
    let q = (h - g).sqrt();
    let s = (if b > 0.0 { f(b) + q } else { f(b) - q }) * -0.5;
    let inv = 1.0 / (f(a) * s);
    let root_b = (inv * s * s) as f32;
    let root_a = (f(inv as f32) * f(a) * f(c)) as f32;
    Some(if root_b.is_nan() || root_b >= root_a {
        root_b
    } else {
        root_a
    })
}

/// The client's integer inverse-square-root estimate, without a Newton step: its error is part of
/// the glow's shape.
fn fast_inv_sqrt(x: f32) -> f32 {
    f32::from_bits(0x5f39_97bbu32.wrapping_sub((x.to_bits() >> 1) & 0x3fff_ffff))
}

fn clear_with_colour_of(left: [u8; 4]) -> [u8; 4] {
    [left[0], left[1], left[2], 0]
}

/// The client's float-to-byte trick: adding 2^9 puts `floor(x)`, for `x` in `0..256`, in the
/// mantissa's bits 14 to 21.
fn float_byte(x: f64) -> u8 {
    (((x + 512.0) as f32).to_bits() >> 14) as u8
}

fn pack_channel(ch: f64) -> u8 {
    let clamped = if ch < 1.0 { ch } else { 1.0 };
    float_byte(clamped * 255.0)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn frame() -> CloudFrame {
        CloudFrame {
            glow: [1.0, 0.78, 0.54],
            slope: [0.17, 0.41, 0.52],
            base: [0.1, 0.1, 0.12],
            glow_dir: Vec3::new(0.6, 0.5, 0.1).normalize(),
            glow_track: 1.0,
        }
    }

    #[test]
    fn a_cloudless_zone_covers_nothing() {
        let mut k = CloudKernel::default();
        k.rebuild(0.0, &frame());
        assert!(k.tile.iter().all(|&b| b == 0));
        assert!(k.rgba().iter().all(|px| px[3] == 0));
    }

    #[test]
    fn density_shapes_the_field_and_it_regenerates_the_same() {
        let (mut a, mut b) = (CloudKernel::default(), CloudKernel::default());
        a.rebuild(1.0, &frame());
        b.rebuild(1.0, &frame());
        assert_eq!((&a.tile, a.rgba()), (&b.tile, b.rgba()));
        assert!(
            a.tile.iter().all(|&v| v > 0),
            "overcast leaves no clear cell"
        );
        let mean = a.tile.iter().map(|&v| u32::from(v)).sum::<u32>() / a.tile.len() as u32;
        assert!(mean > 200, "overcast mean {mean}");
        let mut mid = CloudKernel::default();
        mid.rebuild(0.6, &frame());
        let clear: usize = mid.tile.iter().map(|&v| usize::from(v == 0)).sum();
        let covered = mid.tile.iter().filter(|&&v| v > 100).count();
        assert!(clear > 0 && covered > 0, "clear {clear} covered {covered}");
    }

    #[test]
    fn four_bands_make_a_whole_pass_but_the_first_rows_colours() {
        let mut bands = CloudKernel::default();
        bands.rebuild(0.6, &frame());
        for _ in 0..4 {
            bands.regen_if_due(1.0, 0.6, &frame());
        }
        let mut whole = CloudKernel {
            pass: LatticeKey(1),
            ..CloudKernel::default()
        };
        whole.rebuild(0.6, &frame());
        assert_eq!(bands.tile, whole.tile);
        assert_eq!(bands.rgba()[SIDE..], whole.rgba()[SIDE..]);
    }

    #[test]
    fn colours_follow_the_byte_arithmetic() {
        let mut k = CloudKernel::default();
        let unlit = CloudFrame {
            glow_track: 0.0,
            ..frame()
        };
        k.rebuild(1.0, &unlit);
        for (g, px) in k.rgba().iter().enumerate() {
            let t = k.tile[g];
            let p = f64::from(INV_255) * f64::from(((255 - t) >> 1) + 64);
            let want = |sl: f32, b: f32| {
                pack_channel(f64::from((f64::from(sl) * p + f64::from(b)) as f32))
            };
            assert_eq!(
                *px,
                [
                    want(unlit.slope[0], unlit.base[0]),
                    want(unlit.slope[1], unlit.base[1]),
                    want(unlit.slope[2], unlit.base[2]),
                    t
                ],
                "cell {g}"
            );
        }
        let mut lit = CloudKernel::default();
        lit.rebuild(1.0, &frame());
        let pairs = || lit.rgba().iter().zip(k.rgba());
        assert!(pairs().any(|(a, b)| a[0] > b[0]), "the glow never lit");
        assert!(pairs().all(|(a, b)| a[0] >= b[0]));
    }

    #[test]
    fn a_clear_cell_takes_its_left_neighbours_colour() {
        let mut k = CloudKernel::default();
        k.rebuild(0.6, &frame());
        let g = (0..k.tile.len() - 1)
            .find(|&g| k.tile[g] > 0 && (g + 1) % SIDE != 0)
            .expect("a covered cell with a right neighbour");
        k.tile[g + 1] = 0;
        k.recolor(&frame());
        let (a, b) = (k.rgba()[g], k.rgba()[g + 1]);
        assert_eq!(b, [a[0], a[1], a[2], 0]);
    }

    #[test]
    fn the_inverse_square_root_is_the_clients_estimate() {
        assert_eq!(fast_inv_sqrt(1.0).to_bits(), 0x3f79_97bb);
        assert!((fast_inv_sqrt(4.0) - 0.5).abs() < 0.02);
    }

    #[test]
    fn the_float_trick_floors_a_byte() {
        assert_eq!(INV_255.to_bits(), 0x3b80_8081);
        assert_eq!(
            (float_byte(0.0), float_byte(127.99), float_byte(255.5)),
            (0, 127, 255)
        );
        assert_eq!(pack_channel(2.0), 255);
    }
}
