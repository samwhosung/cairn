use m2::M2Track;

pub(crate) trait Lerp: Copy {
    fn lerp(a: Self, b: Self, f: f32) -> Self;
}
impl Lerp for f32 {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        a + (b - a) * f
    }
}
impl Lerp for [f32; 2] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [f32::lerp(a[0], b[0], f), f32::lerp(a[1], b[1], f)]
    }
}
impl Lerp for [f32; 3] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [
            f32::lerp(a[0], b[0], f),
            f32::lerp(a[1], b[1], f),
            f32::lerp(a[2], b[2], f),
        ]
    }
}

impl Lerp for [f32; 4] {
    fn lerp(a: Self, b: Self, f: f32) -> Self {
        [
            f32::lerp(a[0], b[0], f),
            f32::lerp(a[1], b[1], f),
            f32::lerp(a[2], b[2], f),
            f32::lerp(a[3], b[3], f),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SeqSlot {
    pub file_index: usize,
    pub band_ms: (u32, u32),
    pub looping: bool,
}

/// One baked keyed loop, in seconds, holding the first or last key outside the keyed span. A
/// `period` of `0.0` is a constant.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyAnim<V> {
    /// Seconds: the global sequence's length, or the sequence band's.
    pub period: f32,
    /// Hold each key until the next rather than interpolate.
    pub step: bool,
    /// Time wraps modulo `period` for a global sequence or a looping sequence, and clamps to it
    /// for a non-looping one, which holds its last value once it ends.
    pub wrap: bool,
    /// Runs on the instance's global-sequence clock rather than the sequence's; see
    /// [`Self::clock`].
    pub gseq: bool,
    /// `(seconds from loop start, value)`, time-ascending.
    pub keys: Vec<(f32, V)>,
}

impl<V> KeyAnim<V> {
    /// The time to sample at: `gseq_now` modulo the period for a global-sequence loop, else
    /// `band_t`, the seconds into the playing sequence. `gseq_now` is the instance's
    /// global-sequence clock in seconds, which the client counts from the instance's creation.
    pub fn clock(&self, band_t: f32, gseq_now: f64) -> f32 {
        if self.gseq && self.period > 0.0 {
            (gseq_now % f64::from(self.period)) as f32
        } else {
            band_t
        }
    }

    pub(crate) fn sample_or(&self, elapsed: f32, empty: V) -> V
    where
        V: Lerp,
    {
        let wrap = self.wrap;
        let Some(&(t0, v0)) = self.keys.first() else {
            return empty;
        };
        if self.period <= 0.0 || self.keys.len() == 1 {
            return v0;
        }
        let t = if wrap {
            elapsed.rem_euclid(self.period)
        } else {
            elapsed.clamp(0.0, self.period)
        };
        if t <= t0 {
            return v0;
        }
        let mut k0 = 0;
        for (i, &(tk, _)) in self.keys.iter().enumerate() {
            if tk <= t {
                k0 = i;
            } else {
                break;
            }
        }
        let (ta, va) = self.keys[k0];
        if self.step || k0 + 1 >= self.keys.len() {
            return va;
        }
        let (tb, vb) = self.keys[k0 + 1];
        if tb <= ta {
            return va;
        }
        V::lerp(va, vb, (t - ta) / (tb - ta))
    }
}

/// A track's value at absolute `t_ms`, searching key window `[lo, hi]` as the client does; `None`
/// without keys. A window with `lo >= hi` gives `keys[lo]`. Otherwise `k0` is the last window key
/// at or before `t_ms`, else `lo`: a step track or the last key holds it, and anything else lerps
/// toward `k0 + 1`, even past `hi`. Unlike the client, `lo` and `hi` first clamp to the last key,
/// and the lerp stops at the pair's ends where the client extrapolates.
pub(crate) fn sample_window<V: Lerp>(
    keys: &[(u32, V)],
    step: bool,
    lo: usize,
    hi: usize,
    t_ms: u32,
) -> Option<V> {
    let last = keys.len().checked_sub(1)?;
    let (lo, hi) = (lo.min(last), hi.min(last));
    if lo >= hi {
        return keys.get(lo).map(|&(_, v)| v);
    }
    let mut k0 = lo;
    for (k, &(ts, _)) in keys.iter().enumerate().take(hi + 1).skip(lo) {
        if ts <= t_ms {
            k0 = k;
        } else {
            break;
        }
    }
    let (ta, va) = keys[k0];
    if step || k0 + 1 > last {
        return Some(va);
    }
    let (tb, vb) = keys[k0 + 1];
    if tb <= ta {
        return Some(va);
    }
    let f = ((t_ms as f32 - ta as f32) / (tb as f32 - ta as f32)).clamp(0.0, 1.0);
    Some(V::lerp(va, vb, f))
}

/// One optional baked loop per file sequence slot, in file order.
#[derive(Clone, Debug, PartialEq)]
pub struct SeqLoops<V> {
    per_seq: Vec<Option<KeyAnim<V>>>,
}

impl<V: PartialEq> SeqLoops<V> {
    /// The set from one entry per file sequence slot, or `None` when every entry is `None`.
    pub fn new(per_seq: Vec<Option<KeyAnim<V>>>) -> Option<Self> {
        per_seq
            .iter()
            .any(Option::is_some)
            .then_some(Self { per_seq })
    }

    /// The loop every slot holds, or `None` when any two slots differ, including a slot without
    /// a loop beside one with a loop.
    pub fn uniform(&self) -> Option<&KeyAnim<V>> {
        let first = self.per_seq.first()?;
        self.per_seq
            .iter()
            .all(|s| s == first)
            .then_some(first.as_ref())
            .flatten()
    }

    /// The loop in sequence slot `seq`, or in slot 0 when `seq` is `None` or out of range.
    pub fn seq(&self, seq: Option<usize>) -> Option<&KeyAnim<V>> {
        seq.and_then(|i| self.per_seq.get(i))
            .or_else(|| self.per_seq.first())?
            .as_ref()
    }

    /// Every slot's loop, in file order.
    pub fn slots(&self) -> &[Option<KeyAnim<V>>] {
        &self.per_seq
    }
}

pub(crate) fn bake_track<T: Copy, V: Lerp + PartialEq>(
    track: &M2Track<T>,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
    proj: impl Fn(T) -> V,
    drop_constant: impl Fn(V) -> bool,
    is_identity: impl Fn(V) -> bool,
) -> Option<KeyAnim<V>> {
    if track.keys.is_empty() {
        return None;
    }
    let keys: Vec<(u32, V)> = track.keys.iter().map(|&(t, v)| (t, proj(v))).collect();
    let step = track.interp == 0;
    let constant = |v: V| KeyAnim {
        period: 0.0,
        step,
        wrap: true,
        gseq: false,
        keys: vec![(0.0, v)],
    };
    let (_, first) = keys[0];
    if keys.iter().all(|&(_, v)| v == first) {
        if drop_constant(first) {
            return None;
        }
        return Some(constant(first));
    }
    if track.gseq != 0xffff {
        // Global-sequence keys are already in the loop's own time.
        let period_ms = gseq_durations
            .get(track.gseq as usize)
            .copied()
            .filter(|&d| d > 0)
            .unwrap_or_else(|| keys.last().map_or(0, |&(t, _)| t).max(1));
        return Some(KeyAnim {
            period: period_ms as f32 / 1000.0,
            step,
            wrap: true,
            gseq: true,
            keys: keys.iter().map(|&(t, v)| (t as f32 / 1000.0, v)).collect(),
        });
    }
    let SeqSlot {
        file_index: index,
        band_ms: band,
        looping,
    } = seq?;
    let (start, end) = band;
    // Without ranges the client searches the whole key list.
    let (lo, hi) = match track.ranges.get(index) {
        Some(&(lo, hi)) => (lo as usize, hi as usize),
        None => (0, keys.len().saturating_sub(1)),
    };
    let at = |t: u32| sample_window(&keys, step, lo, hi, t);
    let head = at(start)?;
    if end <= start {
        return (!is_identity(head)).then(|| constant(head));
    }
    let mut baked: Vec<(f32, V)> = vec![(0.0, head)];
    for (k, &(t, v)) in keys.iter().enumerate() {
        if k >= lo && k <= hi && t > start && t < end {
            baked.push(((t - start) as f32 / 1000.0, v));
        }
    }
    if let Some(tail) = at(end) {
        baked.push(((end - start) as f32 / 1000.0, tail));
    }
    if baked.iter().all(|&(_, v)| v == head) {
        return (!is_identity(head)).then(|| constant(head));
    }
    Some(KeyAnim {
        period: ((end - start) as f32 / 1000.0).max(0.001),
        step,
        wrap: looping,
        gseq: false,
        keys: baked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_of(period: f32, keys: &[(f32, f32)]) -> KeyAnim<f32> {
        KeyAnim {
            period,
            step: false,
            wrap: true,
            gseq: false,
            keys: keys.to_vec(),
        }
    }

    #[test]
    fn a_dead_slot_zero_beside_a_live_slot_is_not_uniform() {
        let set = SeqLoops::new(vec![None, Some(loop_of(3.3, &[(0.0, 0.0), (1.0, 0.6)]))])
            .expect("one slot animates");
        assert!(set.uniform().is_none());
        assert!(set.seq(Some(0)).is_none(), "slot 0 is the dead hold");
        assert!(set.seq(Some(1)).is_some(), "slot 1 is the animation");
    }

    #[test]
    fn matching_slots_collapse_to_the_one_shared_loop() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), Some(l.clone())]).expect("animates");
        assert_eq!(set.uniform(), Some(&l));
    }

    #[test]
    fn differing_live_slots_are_not_uniform_either() {
        let set = SeqLoops::new(vec![
            Some(loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)])),
            Some(loop_of(2.0, &[(0.0, 1.0), (2.0, 0.0)])),
        ])
        .expect("animates");
        assert!(set.uniform().is_none());
    }

    #[test]
    fn a_set_with_no_live_slot_is_no_set() {
        assert!(SeqLoops::<f32>::new(vec![None, None]).is_none());
    }

    #[test]
    fn an_unknown_sequence_degrades_to_slot_zero() {
        let l = loop_of(1.0, &[(0.0, 0.0), (1.0, 1.0)]);
        let set = SeqLoops::new(vec![Some(l.clone()), None]).expect("animates");
        assert_eq!(set.seq(None), Some(&l));
        assert_eq!(set.seq(Some(9)), Some(&l));
    }
}
