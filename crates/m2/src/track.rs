use wowfile::ByteExt;

use crate::parse::rd_vec3;

/// One animation track. Keys carry absolute milliseconds on the model's whole timeline; a track
/// tagged with a global sequence loops on that sequence's own clock instead.
#[derive(Clone, Debug)]
pub struct M2Track<V> {
    /// `0` steps to the previous key; anything else interpolates.
    pub interp: u16,
    /// The global sequence index, `0xffff` for a track on the sequence timeline.
    pub gseq: u16,
    /// Per sequence, in file order, the window of key indices the client searches. The window
    /// can reach into a later sequence's keys, so it brackets rather than selects them; a
    /// sequence with no keys of its own resolves to `keys[lo]`.
    pub ranges: Vec<(u32, u32)>,
    /// `(ms, value)` in file order. There are as many as the shorter of the file's timestamp
    /// and value arrays.
    pub keys: Vec<(u32, V)>,
}

impl<V> Default for M2Track<V> {
    fn default() -> Self {
        Self {
            interp: 0,
            gseq: 0,
            ranges: Vec::new(),
            keys: Vec::new(),
        }
    }
}

/// A signed fixed-point scalar track, `i16 / 32767`: colour alpha and transparency weight.
pub type M2ScalarTrack = M2Track<f32>;
/// A three-float track: texture translation and scaling, colour RGB.
pub type M2Vec3Track = M2Track<[f32; 3]>;
/// A four-float quaternion track: texture rotation.
pub type M2QuatTrack = M2Track<[f32; 4]>;

impl<V: Copy + PartialEq> M2Track<V> {
    /// The value, when every key holds the same one; `None` when keyless or varying.
    pub fn constant(&self) -> Option<V> {
        let (_, first) = *self.keys.first()?;
        self.keys.iter().all(|&(_, v)| v == first).then_some(first)
    }
}

/// Reads the track whose 28-byte header is at `track_ofs`. Anything out of range reads as no
/// keys or no ranges: shipped models rely on that.
fn track_read<V>(
    b: &[u8],
    track_ofs: usize,
    val_size: usize,
    read_val: impl Fn(&[u8], usize) -> Option<V>,
) -> M2Track<V> {
    let (Some(interp), Some(gseq)) = (b.u16_at(track_ofs), b.u16_at(track_ofs + 2)) else {
        return M2Track::default();
    };
    let Some(((tn, to), (vn, vo))) = b
        .u32_at(track_ofs + 0x0c)
        .zip(b.u32_at(track_ofs + 0x10))
        .zip(b.u32_at(track_ofs + 0x14).zip(b.u32_at(track_ofs + 0x18)))
    else {
        return M2Track::default();
    };
    let n = tn.min(vn) as usize;
    let (to, vo) = (to as usize, vo as usize);
    let keys = (0..n)
        .map_while(|i| b.u32_at(to + i * 4).zip(read_val(b, vo + i * val_size)))
        .collect();
    let ranges = match b.u32_at(track_ofs + 0x04).zip(b.u32_at(track_ofs + 0x08)) {
        Some((rn, ro)) => (0..rn as usize)
            .map_while(|i| {
                let e = ro as usize + i * 8;
                b.u32_at(e).zip(b.u32_at(e + 4))
            })
            .collect(),
        None => Vec::new(),
    };
    M2Track {
        interp,
        gseq,
        ranges,
        keys,
    }
}

/// The key is signed: the client culls a batch at alpha `<= 0`, and shipped models author
/// `0x8001`, -1.0, to hide one.
pub(crate) fn track_fix16(b: &[u8], track_ofs: usize) -> M2ScalarTrack {
    track_read(b, track_ofs, 2, |b, o| {
        b.u16_at(o).map(|v| f32::from(v as i16) / 32767.0)
    })
}

pub(crate) fn track_vec3_timed(b: &[u8], track_ofs: usize) -> M2Vec3Track {
    track_read(b, track_ofs, 12, rd_vec3)
}

pub(crate) fn track_quat(b: &[u8], track_ofs: usize) -> M2QuatTrack {
    track_read(b, track_ofs, 16, |b, o| {
        Some([
            b.f32_at(o)?,
            b.f32_at(o + 4)?,
            b.f32_at(o + 8)?,
            b.f32_at(o + 12)?,
        ])
    })
}

/// One key of a cubic track: its value and the tangents into and out of it. Keys of a cubic
/// track take this size whatever the track's `interp`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct M2SplineKey<V> {
    pub value: V,
    pub in_tan: V,
    pub out_tan: V,
}

/// A cubic three-float track: camera position and target.
pub type M2Vec3SplineTrack = M2Track<M2SplineKey<[f32; 3]>>;
/// A cubic scalar track: camera roll.
pub type M2ScalarSplineTrack = M2Track<M2SplineKey<f32>>;

/// A value a cubic track can hold.
pub trait CubicValue: Copy {
    /// `w0·p0 + w1·p1 + w2·p2 + w3·p3`.
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self;
    /// `a + (b − a)·t`, the client's form, which rounds differently from `(1−t)·a + t·b`.
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl CubicValue for f32 {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        w[0] * p[0] + w[1] * p[1] + w[2] * p[2] + w[3] * p[3]
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl CubicValue for [f32; 3] {
    fn combine(w: [f32; 4], p: [Self; 4]) -> Self {
        std::array::from_fn(|j| w[0] * p[0][j] + w[1] * p[1][j] + w[2] * p[2][j] + w[3] * p[3][j])
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        std::array::from_fn(|j| a[j] + (b[j] - a[j]) * t)
    }
}

impl<V: CubicValue> M2Track<M2SplineKey<V>> {
    /// Samples the track at `ms`, holding the first and last keys beyond the ends. `interp`
    /// picks step (`0`), linear (`1`), Bézier (`2`) over `value[k0], out_tan[k0], in_tan[k1],
    /// value[k1]`, or Hermite (`3` and up) with `out_tan[k0]` and `in_tan[k1]`.
    pub fn sample_ms(&self, ms: u32) -> Option<V> {
        let first = self.keys.first()?;
        let last = self.keys.last()?;
        if ms <= first.0 {
            return Some(first.1.value);
        }
        if ms >= last.0 {
            return Some(last.1.value);
        }
        let k1 = self.keys.partition_point(|&(t, _)| t <= ms);
        let (t0, a) = self.keys[k1 - 1];
        let (t1, b) = self.keys[k1];
        let t = if t1 > t0 {
            (ms - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        let (t2, t3) = (t * t, t * t * t);
        Some(match self.interp {
            0 => a.value,
            1 => V::lerp(a.value, b.value, t),
            2 => V::combine(
                [
                    (1.0 - t) * (1.0 - t) * (1.0 - t),
                    3.0 * t * (1.0 - t) * (1.0 - t),
                    3.0 * t2 * (1.0 - t),
                    t3,
                ],
                [a.value, a.out_tan, b.in_tan, b.value],
            ),
            _ => V::combine(
                [
                    2.0 * t3 - 3.0 * t2 + 1.0,
                    t3 - 2.0 * t2 + t,
                    3.0 * t2 - 2.0 * t3,
                    t3 - t2,
                ],
                [a.value, a.out_tan, b.value, b.in_tan],
            ),
        })
    }
}

fn rd_spline<V>(
    b: &[u8],
    o: usize,
    step: usize,
    rd: impl Fn(&[u8], usize) -> Option<V>,
) -> Option<M2SplineKey<V>> {
    Some(M2SplineKey {
        value: rd(b, o)?,
        in_tan: rd(b, o + step)?,
        out_tan: rd(b, o + 2 * step)?,
    })
}

pub(crate) fn track_spline_vec3(b: &[u8], track_ofs: usize) -> M2Vec3SplineTrack {
    track_read(b, track_ofs, 0x24, |b, o| rd_spline(b, o, 12, rd_vec3))
}

pub(crate) fn track_spline_f32(b: &[u8], track_ofs: usize) -> M2ScalarSplineTrack {
    track_read(b, track_ofs, 0xc, |b, o| rd_spline(b, o, 4, <[u8]>::f32_at))
}
