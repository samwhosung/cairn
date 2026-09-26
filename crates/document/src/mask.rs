//! Where within its outline a stroke or a scatter lands: limits on the ground's slope and height
//! and the distance from water, each easing over `soft` at its ends, and on the textures covering
//! the ground.

use crate::command::Words;
use crate::frame::{CELL, CHUNK, TEXEL};
use crate::grammar::Args;
use crate::install::Install;
use crate::shape::smoothstep;
use crate::text::number;
use crate::zone::{TEXELS_ACROSS, Zone};

pub const FLAGS: [&str; 5] = ["height", "water", "on", "off", "soft"];

/// Values from `lo` to `hi`, both included; a missing end is unbounded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub lo: Option<f64>,
    pub hi: Option<f64>,
}

impl Span {
    pub const ANY_SLOPE: Span = Span {
        lo: Some(0.0),
        hi: Some(90.0),
    };

    /// `LO..HI`, `LO..` or `..HI`.
    pub fn parse(s: &str, what: &str) -> Result<Span, String> {
        let want = || format!("{what} {s:?}: want LO..HI, LO.. or ..HI");
        let (lo, hi) = s.split_once("..").ok_or_else(want)?;
        let end = |v: &str| -> Result<Option<f64>, String> {
            if v.is_empty() {
                return Ok(None);
            }
            let n: f64 = number(v, what)?;
            if n.is_finite() {
                Ok(Some(n))
            } else {
                Err(want())
            }
        };
        let span = Span {
            lo: end(lo)?,
            hi: end(hi)?,
        };
        match (span.lo, span.hi) {
            (None, None) => Err(want()),
            (Some(a), Some(b)) if a > b => {
                Err(format!("{what} {s:?}: its low end is above its high"))
            }
            _ => Ok(span),
        }
    }

    pub fn text(&self) -> String {
        let end = |v: Option<f64>| v.map_or(String::new(), |v| v.to_string());
        format!("{}..{}", end(self.lo), end(self.hi))
    }

    /// 1 inside, 0 outside, and across `soft` centred on each end, easing between.
    pub fn weight(&self, v: f64, soft: f64) -> f64 {
        let inside_by = |by: f64| {
            if soft > 0.0 {
                smoothstep(by / soft + 0.5)
            } else if by >= 0.0 {
                1.0
            } else {
                0.0
            }
        };
        self.lo.map_or(1.0, |lo| inside_by(v - lo)) * self.hi.map_or(1.0, |hi| inside_by(hi - v))
    }

    fn settles_beyond(&self, soft: f64) -> f64 {
        [self.lo, self.hi]
            .into_iter()
            .flatten()
            .map(f64::abs)
            .fold(0.0, f64::max)
            + soft / 2.0
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mask {
    pub height: Option<Span>,
    /// Yards from the water's edge, less than 0 in the water.
    pub water: Option<Span>,
    /// Only where these textures cover the ground: fully where they cover three quarters of it or
    /// more, not where a quarter or less.
    pub on: Vec<String>,
    /// Only where these don't, as `on` eases.
    pub off: Vec<String>,
    /// Degrees of slope, and yards of height and of distance from water, over which an end eases.
    pub soft: f64,
}

impl Mask {
    pub(crate) fn parse(a: &Args) -> Result<Mask, String> {
        let span = |flag: &str, what: &str| a.one(flag)?.map(|s| Span::parse(s, what)).transpose();
        let textures = |flag: &str| -> Result<Vec<String>, String> {
            match a.words(flag) {
                None => Ok(Vec::new()),
                Some([]) => Err(format!("--{flag} TEXTURE ...: which textures")),
                Some(w) => Ok(w.iter().map(|t| t.replace('/', "\\")).collect()),
            }
        };
        let soft = a.num("soft")?.unwrap_or(0.0);
        if !(soft >= 0.0 && soft.is_finite()) {
            return Err("--soft is 0 (a hard edge) or more".into());
        }
        Ok(Mask {
            height: span("height", "--height")?,
            water: span("water", "--water")?,
            on: textures("on")?,
            off: textures("off")?,
            soft,
        })
    }

    pub(crate) fn words(&self, w: &mut Words) {
        for (flag, span) in [("height", self.height), ("water", self.water)] {
            if let Some(s) = span {
                w.flag(flag, [s.text()]);
            }
        }
        for (flag, list) in [("on", &self.on), ("off", &self.off)] {
            if !list.is_empty() {
                w.flag(flag, list.iter().cloned());
            }
        }
        if self.soft > 0.0 {
            w.num_flag("soft", self.soft);
        }
    }
}

/// A mask read against a zone as a command finds it: the textures it names by their place in the
/// palette, and the cells water wets.
pub struct Masking<'a> {
    mask: &'a Mask,
    on: Option<Vec<u16>>,
    off: Vec<u16>,
    shore: Option<Shore>,
}

struct Shore {
    wet: Vec<bool>,
    cells_to_other_kind: Vec<u32>,
}

impl<'a> Masking<'a> {
    pub fn new(
        zone: &Zone,
        mask: &'a Mask,
        install: &mut dyn Install,
    ) -> Result<Masking<'a>, String> {
        let mut places = |list: &[String]| -> Result<Vec<u16>, String> {
            let mut out = Vec::new();
            for t in list {
                match zone.palette_place(t) {
                    Some(p) => out.push(p),
                    None if install.has_texture(t)? => {}
                    None => return Err(format!("{t}: no such texture in the install")),
                }
            }
            Ok(out)
        };
        let on = if mask.on.is_empty() {
            None
        } else {
            Some(places(&mask.on)?)
        };
        Ok(Masking {
            mask,
            on,
            off: places(&mask.off)?,
            shore: mask.water.map(|_| Shore::of(zone)),
        })
    }

    /// How much of a stroke or a scatter the mask, and `slope` when it is given, let land at `p`
    /// in `z`, the zone it was read against, 0 to 1.
    pub fn weight(&self, z: &Zone, p: [f64; 2], slope: Option<&Span>) -> f64 {
        let (m, soft) = (self.mask, self.mask.soft);
        let mut w = 1.0;
        if let Some(s) = slope {
            w *= s.weight(z.heights.slope(p), soft);
        }
        if let Some(s) = &m.height {
            w *= s.weight(z.heights.ground(p), soft);
        }
        let covered = |share: f64| smoothstep((share - 0.25) / 0.5);
        if let (Some(on), true) = (&self.on, w > 0.0) {
            w *= covered(share_covered(z, p, on));
        }
        if w > 0.0 && !self.off.is_empty() {
            w *= 1.0 - covered(share_covered(z, p, &self.off));
        }
        if let (Some(s), Some(shore), true) = (&m.water, &self.shore, w > 0.0) {
            w *= s.weight(
                shore.yards_from_the_edge(z, p, s.settles_beyond(soft)),
                soft,
            );
        }
        w
    }
}

fn share_covered(z: &Zone, p: [f64; 2], textures: &[u16]) -> f64 {
    let (east, south) = z.frame.chunks();
    let gx = ((p[0] / CHUNK).floor().max(0.0) as usize).min(east - 1);
    let gy = ((p[1] / CHUNK).floor().max(0.0) as usize).min(south - 1);
    let texel = |v: f64, g: usize| {
        (((v - g as f64 * CHUNK) / TEXEL).floor().max(0.0) as usize).min(TEXELS_ACROSS - 1)
    };
    let t = texel(p[1], gy) * TEXELS_ACROSS + texel(p[0], gx);
    let sum: u32 = z.paint[gy * east + gx]
        .layers
        .iter()
        .filter(|l| textures.contains(&l.palette_place))
        .map(|l| u32::from(l.w[t]))
        .sum();
    f64::from(sum.min(255)) / 255.0
}

impl Shore {
    fn of(z: &Zone) -> Shore {
        let (cols, rows) = z.frame.cells();
        let wet = wet_cells(z);
        let far = u32::MAX / 2;
        let mut other: Vec<u32> = (0..cols * rows)
            .map(|k| {
                let (i, j) = (k % cols, k / cols);
                let to_edge = i.min(j).min(cols - 1 - i).min(rows - 1 - j) as u32 + 1;
                if wet[k] { to_edge } else { far }
            })
            .collect();
        for backwards in [false, true] {
            for n in 0..cols * rows {
                let k = if backwards { cols * rows - 1 - n } else { n };
                let (i, j) = ((k % cols) as i64, (k / cols) as i64);
                let mut best = other[k];
                for (di, dj) in [(-1i64, -1i64), (0, -1), (1, -1), (-1, 0)] {
                    let (di, dj) = if backwards { (-di, -dj) } else { (di, dj) };
                    let (ni, nj) = (i + di, j + dj);
                    if ni < 0 || nj < 0 || ni >= cols as i64 || nj >= rows as i64 {
                        continue;
                    }
                    let q = nj as usize * cols + ni as usize;
                    let through = if wet[q] == wet[k] {
                        other[q].saturating_add(1)
                    } else {
                        1
                    };
                    best = best.min(through);
                }
                other[k] = best;
            }
        }
        Shore {
            wet,
            cells_to_other_kind: other,
        }
    }

    fn yards_from_the_edge(&self, z: &Zone, p: [f64; 2], enough: f64) -> f64 {
        let (cols, rows) = z.frame.cells();
        let ci = ((p[0] / CELL).floor().max(0.0) as i64).min(cols as i64 - 1);
        let cj = ((p[1] / CELL).floor().max(0.0) as i64).min(rows as i64 - 1);
        let k = cj as usize * cols + ci as usize;
        let inside = self.wet[k];
        let sign = if inside { -1.0 } else { 1.0 };
        let rings = (enough / CELL).ceil() as i64 + 1;
        if i64::from(self.cells_to_other_kind[k]) > rings {
            return sign * f64::INFINITY;
        }
        let wet_at = |i: i64, j: i64| {
            (0..cols as i64).contains(&i)
                && (0..rows as i64).contains(&j)
                && self.wet[j as usize * cols + i as usize]
        };
        let gap = |v: f64, k: i64| {
            (k as f64 * CELL - v)
                .max(v - (k + 1) as f64 * CELL)
                .max(0.0)
        };
        let mut best = f64::MAX;
        for ring in 0..=rings {
            if best <= (ring - 1).max(0) as f64 * CELL {
                break;
            }
            for dj in -ring..=ring {
                for di in -ring..=ring {
                    let (i, j) = (ci + di, cj + dj);
                    if di.abs().max(dj.abs()) == ring && wet_at(i, j) != inside {
                        best = best.min(gap(p[0], i).hypot(gap(p[1], j)));
                    }
                }
            }
        }
        sign * best
    }
}

fn wet_cells(z: &Zone) -> Vec<bool> {
    let (cols, rows) = z.frame.cells();
    let mut wet = vec![false; cols * rows];
    for w in z.water.values() {
        let [lo, hi] = w.shape.bounds();
        let cell = |v: f64, n: usize| ((v / CELL).floor().max(0.0) as usize).min(n - 1);
        for j in cell(lo[1], rows)..=cell(hi[1], rows) {
            for i in cell(lo[0], cols)..=cell(hi[0], cols) {
                if crate::build::wets(w, &z.heights, i, j) {
                    wet[j * cols + i] = true;
                }
            }
        }
    }
    wet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammar::split;

    #[test]
    fn a_span_eases_over_soft_centred_on_its_ends() {
        let s = Span::parse("30..", "--slope").expect("a span");
        let is = |v: f64, want: f64| (v - want).abs() < 1e-12;
        assert!(is(s.weight(29.9, 0.0), 0.0) && is(s.weight(30.0, 0.0), 1.0));
        assert!(is(s.weight(30.0, 4.0), 0.5));
        assert!(is(s.weight(28.0, 4.0), 0.0) && is(s.weight(32.0, 4.0), 1.0));
        let both = Span::parse("-5..10", "--water").expect("a span");
        assert!(is(both.weight(-6.0, 0.0), 0.0) && is(both.weight(11.0, 0.0), 0.0));
        assert!(is(both.weight(0.0, 2.0), 1.0));
        for bad in ["..", "5", "9..3", "a..b"] {
            assert!(Span::parse(bad, "--height").is_err(), "{bad}");
        }
        for text in ["30..", "..45.5", "-10..0"] {
            assert_eq!(
                Span::parse(text, "x").map(|s| s.text()),
                Ok(text.to_owned())
            );
        }
    }

    fn lake() -> Zone {
        use crate::frame::Frame;
        use crate::shape::Shape;
        use crate::zone::{Texture, Water};
        let mut z = Zone::new(
            "Shore",
            Frame {
                origin: (32, 48),
                size: (1, 1),
            },
            40.0,
            Texture {
                path: "t.blp".into(),
                effect: 0,
            },
        );
        let lake = Water {
            level: 4100,
            shape: Shape::Poly {
                pts: vec![
                    [100.0, 100.0],
                    [300.0, 120.0],
                    [260.0, 330.0],
                    [90.0, 280.0],
                ],
            },
        };
        z.water.insert(
            crate::zone::Id {
                author: "sam".into(),
                n: 1,
            },
            lake,
        );
        z
    }

    #[test]
    fn the_distance_from_water_is_to_the_nearest_wet_cell_or_dry_one() {
        let z = lake();
        let shore = Shore::of(&z);
        let (cols, rows) = z.frame.cells();
        let brute = |p: [f64; 2]| {
            let inside = shore.wet[(p[1] / CELL) as usize * cols + (p[0] / CELL) as usize];
            let mut best = f64::MAX;
            for j in -1..=rows as i64 {
                for i in -1..=cols as i64 {
                    let wet = (0..cols as i64).contains(&i)
                        && (0..rows as i64).contains(&j)
                        && shore.wet[j as usize * cols + i as usize];
                    if wet != inside {
                        let gap = |v: f64, k: i64| {
                            (k as f64 * CELL - v)
                                .max(v - (k + 1) as f64 * CELL)
                                .max(0.0)
                        };
                        best = best.min(gap(p[0], i).hypot(gap(p[1], j)));
                    }
                }
            }
            if inside { -best } else { best }
        };
        let mut tried = 0;
        for k in 0..400 {
            let p = [
                f64::from(k % 20) * 26.3 + 3.1,
                f64::from(k / 20) * 26.1 + 5.7,
            ];
            let (want, got) = (brute(p), shore.yards_from_the_edge(&z, p, 30.0));
            if want.abs() <= 30.0 {
                assert!((want - got).abs() < 1e-9, "{p:?}: {got} for {want}");
                tried += 1;
            } else {
                assert!(
                    got.abs() > 30.0 && got.signum() == want.signum(),
                    "{p:?}: {got}"
                );
            }
        }
        assert!(tried > 50 && shore.wet.iter().any(|&w| w));
    }

    #[test]
    fn a_hard_edged_water_mask_keeps_to_its_far_end() {
        let z = lake();
        let weight = |words: &str, p: [f64; 2]| {
            let m = Mask::parse(&Args::parse(&split(words).expect("words"))).expect("a mask");
            let masking = Masking::new(&z, &m, &mut crate::tests::FakeInstall).expect("a masking");
            masking.weight(&z, p, None)
        };
        let (deep, near_in, near_out, far) =
            ([200.0, 220.0], [97.5, 200.0], [93.0, 200.0], [20.0, 480.0]);
        let shore = Shore::of(&z);
        assert!((-3.0..0.0).contains(&shore.yards_from_the_edge(&z, near_in, 5.0)));
        assert!((0.0..5.0).contains(&shore.yards_from_the_edge(&z, near_out, 5.0)));
        for (p, want) in [(deep, 1.0), (near_in, 1.0), (near_out, 1.0), (far, 0.0)] {
            assert!(
                (weight("--water ..5", p) - want).abs() < 1e-12,
                "..5 at {p:?}"
            );
        }
        for (p, want) in [(deep, 0.0), (near_in, 1.0), (near_out, 1.0), (far, 1.0)] {
            assert!(
                (weight("--water -3..", p) - want).abs() < 1e-12,
                "-3.. at {p:?}"
            );
        }
    }

    #[test]
    fn a_mask_reads_back_from_its_words() {
        let words = split("--height ..48 --water 2.. --on A.blp B.blp --off C.blp --soft 4")
            .expect("words");
        let m = Mask::parse(&Args::parse(&words)).expect("a mask");
        let mut w = Words::default();
        m.words(&mut w);
        assert_eq!(Mask::parse(&Args::parse(&w.0)), Ok(m));
    }
}
