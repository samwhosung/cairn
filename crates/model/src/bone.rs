/// How a billboarded M2 bone turns to face the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BillboardKind {
    /// Faces the camera fully.
    Spherical,
    /// Keeps the bone's X axis and turns about it.
    LockX,
    /// Keeps the bone's Y axis and turns about it.
    LockY,
    /// Keeps the bone's Z axis as posed and turns about it.
    LockZ,
}

impl BillboardKind {
    /// The billboard an M2 bone's flags author, `None` for an ordinary bone.
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits & 0x08 != 0 {
            Some(Self::Spherical)
        } else if bits & 0x10 != 0 {
            Some(Self::LockX)
        } else if bits & 0x20 != 0 {
            Some(Self::LockY)
        } else if bits & 0x40 != 0 {
            Some(Self::LockZ)
        } else {
            None
        }
    }
}

/// How an M2 bone's parent matrix is rebuilt from the model's root matrix before the bone
/// composes: bone flags `0x1`, `0x2` and `0x4` ignore the parent's translation, scale and
/// rotation. A billboard on the same bone then applies to the rebuilt matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParentArm {
    /// `0x1`: the translation is the root's, not where the parent carried the bone.
    pub ignore_translate: bool,
    pub basis: ParentBasis,
}

/// What `flags & 0x6` does to the three basis vectors of the parent matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParentBasis {
    Keep,
    /// Ignores the parent's scale: each vector is normalized, its direction kept.
    UnitNormalize,
    /// Ignores the parent's rotation: the root's directions at the parent's lengths.
    RootDirection,
    /// Ignores the parent's rotation and scale: the root's basis as it is.
    RootBasis,
}

impl ParentArm {
    /// The arm an M2 bone's flags author, `None` when none of `0x1`, `0x2` and `0x4` is set.
    pub fn from_bone_flags(bits: u32) -> Option<Self> {
        if bits.trailing_zeros() >= 3 {
            return None;
        }
        Some(Self {
            ignore_translate: bits & 0x1 != 0,
            basis: match bits & 0x6 {
                0x2 => ParentBasis::UnitNormalize,
                0x4 => ParentBasis::RootDirection,
                0x6 => ParentBasis::RootBasis,
                _ => ParentBasis::Keep,
            },
        })
    }
}

/// A looping bone track of three-component keys on a millisecond clock: a bone's
/// global-sequence scale, or its translation within one sequence.
#[derive(Debug, Clone)]
pub struct BoneScaleAnim {
    pub duration_ms: u32,
    /// Linear interpolation between keys; `false` steps.
    pub interp: bool,
    /// `(time_ms, value)` keys, ascending in time.
    pub keys: Vec<(u32, [f32; 3])>,
}

impl BoneScaleAnim {
    /// The value at `time_ms` wrapped into the loop: the first key's value up to its time, the
    /// last key's after it, and `[1.0; 3]` when there are no keys.
    pub fn sample(&self, time_ms: u32) -> [f32; 3] {
        let n = self.keys.len();
        if n == 0 {
            return [1.0; 3];
        }
        let t = time_ms % self.duration_ms.max(1);
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1;
        }
        let (t0, v0) = self.keys[k];
        let (t1, v1) = self.keys[k + 1];
        let frac = if t1 > t0 {
            (t - t0) as f32 / (t1 - t0) as f32
        } else {
            0.0
        };
        [
            v0[0] + (v1[0] - v0[0]) * frac,
            v0[1] + (v1[1] - v0[1]) * frac,
            v0[2] + (v1[2] - v0[2]) * frac,
        ]
    }
}

/// A parentless bone whose sequence keys rotation alone, so the vertices wholly weighted to it
/// turn rigidly about its pivot: `T(pivot) · R(t) · T(−pivot)`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneSpin {
    /// Model space.
    pub pivot: [f32; 3],
    /// The sequence length in seconds.
    pub duration: f32,
    /// Spherical interpolation between keys; `false` steps.
    pub interp: bool,
    /// `(seconds, [x, y, z, w])` keys from the sequence start, ascending in time, rotating in
    /// model space.
    pub keys: Vec<(f32, [f32; 4])>,
}

impl BoneSpin {
    /// The rotation at `time` seconds wrapped into `[0, duration)`, negative times included: the
    /// first key's up to its time, the last key's after it, and the identity with no keys.
    pub fn sample(&self, time: f32) -> [f32; 4] {
        const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        let n = self.keys.len();
        if n == 0 {
            return IDENTITY;
        }
        let t = if self.duration > 0.0 {
            time.rem_euclid(self.duration)
        } else {
            0.0
        };
        if t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        let mut k = 0;
        while k + 1 < n && self.keys[k + 1].0 <= t {
            k += 1;
        }
        if !self.interp || k + 1 >= n {
            return self.keys[k].1;
        }
        let (t0, q0) = self.keys[k];
        let (t1, q1) = self.keys[k + 1];
        let frac = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        slerp(q0, q1, frac)
    }
}

fn slerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut dot = (0..4).map(|i| a[i] * b[i]).sum::<f32>();
    let mut b = b;
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }
    let (w0, w1) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.clamp(-1.0, 1.0).acos();
        let sin = theta.sin();
        (((1.0 - t) * theta).sin() / sin, (t * theta).sin() / sin)
    };
    let mut q = [0.0f32; 4];
    for i in 0..4 {
        q[i] = a[i] * w0 + b[i] * w1;
    }
    let len = q.iter().map(|c| c * c).sum::<f32>().sqrt();
    if len > 0.0 {
        for c in &mut q {
            *c /= len;
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    fn about_z(deg: f32) -> [f32; 4] {
        let h = deg.to_radians() * 0.5;
        [0.0, 0.0, h.sin(), h.cos()]
    }

    fn angle_deg(q: [f32; 4]) -> f32 {
        2.0 * q[3].abs().clamp(0.0, 1.0).acos().to_degrees()
    }

    fn spin(keys: Vec<(f32, [f32; 4])>, duration: f32, interp: bool) -> BoneSpin {
        BoneSpin {
            pivot: [0.0; 3],
            duration,
            interp,
            keys,
        }
    }

    #[test]
    fn a_spin_slerps_clamps_and_wraps() {
        let s = spin(vec![(0.0, about_z(0.0)), (2.0, about_z(90.0))], 4.0, true);
        assert!(angle_deg(s.sample(0.0)) < 1e-3);
        assert!(
            (angle_deg(s.sample(-1.0)) - 90.0).abs() < 1e-3,
            "wraps below zero"
        );
        assert!((angle_deg(s.sample(1.0)) - 45.0).abs() < 1e-2);
        assert!(
            (angle_deg(s.sample(3.0)) - 90.0).abs() < 1e-3,
            "holds the last key"
        );
        assert!(
            (angle_deg(s.sample(5.0)) - 45.0).abs() < 1e-2,
            "wraps past the end"
        );
    }

    #[test]
    fn a_step_spin_holds_its_key() {
        let s = spin(vec![(0.0, about_z(0.0)), (2.0, about_z(90.0))], 4.0, false);
        assert!(angle_deg(s.sample(1.9)) < 1e-3);
        assert!((angle_deg(s.sample(2.0)) - 90.0).abs() < 1e-3);
    }

    #[test]
    fn a_spin_takes_the_short_way_round() {
        let s = spin(
            vec![(0.0, about_z(350.0)), (1.0, about_z(360.0))],
            1.0,
            true,
        );
        let mid = angle_deg(s.sample(0.5));
        assert!(!(15.0..=345.0).contains(&mid), "{mid}");
    }
}
