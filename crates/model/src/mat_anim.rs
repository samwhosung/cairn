use m2::{M2ScalarTrack, M2Vec3Track};

use crate::key_anim::{KeyAnim, SeqLoops, SeqSlot, bake_track};

/// A baked colour-alpha or transparency-weight loop.
pub type ScalarAnim = KeyAnim<f32>;

impl KeyAnim<f32> {
    /// The value `elapsed` seconds in, on the loop's clock; `1.0` without keys.
    pub fn sample(&self, elapsed: f32) -> f32 {
        self.sample_or(elapsed, 1.0)
    }
}

/// A baked RGB tint loop, from a colour record's colour track.
pub type RgbAnim = KeyAnim<[f32; 3]>;

impl KeyAnim<[f32; 3]> {
    /// The tint `elapsed` seconds in, on the loop's clock; white without keys.
    pub fn sample(&self, elapsed: f32) -> [f32; 3] {
        self.sample_or(elapsed, [1.0, 1.0, 1.0])
    }
}

/// A batch's colour-alpha and transparency-weight loops in one sequence; a missing one counts
/// as `1.0`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AlphaSeq {
    pub color: Option<ScalarAnim>,
    pub weight: Option<ScalarAnim>,
}

impl AlphaSeq {
    /// The product of both factors `elapsed` seconds into the sequence, a global-sequence factor
    /// reading `shared_now` instead (see [`KeyAnim::clock`]).
    pub fn sample(&self, elapsed: f32, shared_now: f64) -> f32 {
        let at = |a: &ScalarAnim| a.sample(a.clock(elapsed, shared_now));
        let c = self.color.as_ref().map_or(1.0, at);
        let w = self.weight.as_ref().map_or(1.0, at);
        c * w
    }

    fn is_empty(&self) -> bool {
        self.color.is_none() && self.weight.is_none()
    }
}

/// A batch's animated alpha: one [`AlphaSeq`] per file sequence slot, the slot
/// [`crate::ModelAnimation::seq_index`] names. The client multiplies it by the instance's alpha
/// and skips the batch at zero or below; an opaque batch draws blended while it is below 1.
#[derive(Clone, Debug, PartialEq)]
pub struct AlphaAnim {
    per_seq: Vec<AlphaSeq>,
}

impl AlphaAnim {
    /// The set from one entry per file sequence slot, or `None` when no slot has either factor.
    pub fn new(per_seq: Vec<AlphaSeq>) -> Option<Self> {
        per_seq
            .iter()
            .any(|s| !s.is_empty())
            .then_some(Self { per_seq })
    }

    /// The factors in sequence slot `seq`, or in slot 0 when `seq` is `None` or out of range.
    pub fn seq(&self, seq: Option<usize>) -> &AlphaSeq {
        const IDENTITY: &AlphaSeq = &AlphaSeq {
            color: None,
            weight: None,
        };
        seq.and_then(|i| self.per_seq.get(i))
            .or_else(|| self.per_seq.first())
            .unwrap_or(IDENTITY)
    }

    /// [`AlphaSeq::sample`] in sequence slot `seq`, chosen as [`Self::seq`] chooses it.
    pub fn sample(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> f32 {
        self.seq(seq).sample(elapsed, shared_now)
    }

    /// Every slot's factors, in file order.
    pub fn slots(&self) -> &[AlphaSeq] {
        &self.per_seq
    }

    /// Whether any key of either factor, in any slot, is zero or below.
    pub fn ever_hides(&self) -> bool {
        self.per_seq.iter().any(|s| {
            [s.color.as_ref(), s.weight.as_ref()]
                .into_iter()
                .flatten()
                .any(|a| a.keys.iter().any(|&(_, v)| v <= 0.0))
        })
    }
}

pub(crate) fn bake_scalar_anim(
    track: &M2ScalarTrack,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
) -> Option<ScalarAnim> {
    bake_track(
        track,
        gseq_durations,
        seq,
        |v| v,
        |c| (c - 1.0).abs() < f32::EPSILON || c <= 0.0,
        |v| (v - 1.0).abs() < f32::EPSILON,
    )
}

pub(crate) fn bake_rgb_anim(
    track: &M2Vec3Track,
    gseq_durations: &[u32],
    seq: Option<SeqSlot>,
) -> Option<RgbAnim> {
    bake_track(track, gseq_durations, seq, |v| v, |_| true, |_| true)
}

pub(crate) fn bake_rgb_seqs(
    track: &M2Vec3Track,
    gseq_durations: &[u32],
    slots: &[SeqSlot],
) -> Option<SeqLoops<[f32; 3]>> {
    SeqLoops::new(
        slots
            .iter()
            .map(|&slot| bake_track(track, gseq_durations, Some(slot), |v| v, |_| true, |_| true))
            .collect(),
    )
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn track(gseq: u16, interp: u16, keys: &[(u32, f32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq,
            ranges: Vec::new(),
            keys: keys.to_vec(),
        }
    }

    fn ranged(interp: u16, keys: &[(u32, f32)], ranges: &[(u32, u32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq: 0xffff,
            ranges: ranges.to_vec(),
            keys: keys.to_vec(),
        }
    }

    fn bake_at(t: &M2ScalarTrack, index: usize, band: (u32, u32)) -> Option<ScalarAnim> {
        bake_scalar_anim(
            t,
            &[],
            Some(SeqSlot {
                file_index: index,
                band_ms: band,
                looping: true,
            }),
        )
    }

    fn bake_at_clamped(t: &M2ScalarTrack, index: usize, band: (u32, u32)) -> Option<ScalarAnim> {
        bake_scalar_anim(
            t,
            &[],
            Some(SeqSlot {
                file_index: index,
                band_ms: band,
                looping: false,
            }),
        )
    }

    #[test]
    fn bake_keeps_only_what_the_static_path_cannot_do() {
        assert_eq!(bake_scalar_anim(&track(0xffff, 1, &[]), &[], None), None);
        assert_eq!(
            bake_scalar_anim(&track(0xffff, 1, &[(0, 1.0)]), &[], None),
            None
        );
        assert_eq!(
            bake_scalar_anim(&track(0xffff, 1, &[(0, 0.0), (500, 0.0)]), &[], None),
            None
        );
        let dim = bake_scalar_anim(&track(0xffff, 1, &[(0, 0.4)]), &[], None).expect("dims");
        assert_eq!(dim.period, 0.0);
        assert_eq!(dim.sample(123.0), 0.4);
    }

    #[test]
    fn gseq_track_wraps_the_table_duration() {
        let a = bake_scalar_anim(
            &track(1, 1, &[(0, 0.2), (750, 1.0), (1500, 0.2)]),
            &[9999, 1500],
            None,
        )
        .expect("animates");
        assert_eq!(a.period, 1.5);
        assert!((a.sample(0.375) - 0.6).abs() < 1e-4);
        assert!((a.sample(1.5 + 0.375) - 0.6).abs() < 1e-4);
    }

    #[test]
    fn gseq_loops_read_the_shared_clock() {
        let g = bake_scalar_anim(
            &track(1, 1, &[(0, 0.2), (750, 1.0), (1500, 0.2)]),
            &[9999, 1500],
            None,
        )
        .expect("animates");
        assert!(g.gseq);
        assert!((g.sample(g.clock(0.0, 1500.375)) - 0.6).abs() < 1e-3);
        let b = ranged(1, &[(1000, 0.0), (1500, 1.0)], &[(0, 1)]);
        let b = bake_at(&b, 0, (1000, 2000)).expect("animates");
        assert!(!b.gseq);
        assert_eq!(
            b.clock(0.25, 999.0),
            0.25,
            "a band loop keeps its own clock"
        );
    }

    #[test]
    fn sequence_track_bakes_one_band_at_a_time() {
        let t = ranged(
            1,
            &[(1000, 0.0), (1500, 1.0), (5000, 0.3)],
            &[(0, 1), (1, 2)],
        );
        let a = bake_at(&t, 0, (1000, 2000)).expect("animates");
        assert_eq!(a.period, 1.0);
        assert!((a.sample(0.25) - 0.5).abs() < 1e-6);
        let expect = 1.0 + (0.3 - 1.0) * (1900.0 - 1500.0) / (5000.0 - 1500.0);
        assert!(
            (a.sample(0.9) - expect).abs() < 1e-4,
            "past its window, band 0 lerps toward band 1's key: got {} want {expect}",
            a.sample(0.9)
        );
        let b = bake_at(&t, 1, (4000, 5000)).expect("animates");
        assert!(
            (b.sample(0.0) - 0.5).abs() < 1e-4,
            "band 1 opens partway down the ramp: got {}",
            b.sample(0.0)
        );
        assert!(
            (b.sample(0.999) - 0.3).abs() < 1e-3,
            "got {}",
            b.sample(0.999)
        );
    }

    #[test]
    fn each_sequence_bakes_its_own_band() {
        let t = ranged(
            1,
            &[(1000, 0.0), (2000, 0.0), (4000, 1.0), (5000, 1.0)],
            &[(0, 1), (2, 3)],
        );
        let stand = bake_at(&t, 0, (1000, 2000)).expect("hidden");
        assert_eq!(stand.sample(0.0), 0.0, "hidden for the whole first band");
        assert_eq!(stand.sample(0.9), 0.0);
        let death = bake_at(&t, 1, (4000, 5000));
        assert_eq!(death, None, "the second band is a plain visible batch");
    }

    #[test]
    fn band_empty_track_holds_its_bracket_key() {
        let t = ranged(1, &[(100, 0.0), (5000, 1.0)], &[(0, 1), (0, 0)]);
        let a = bake_at(&t, 1, (1000, 2000)).expect("holds 0");
        assert_eq!(a.period, 0.0);
        assert_eq!(a.sample(42.0), 0.0);
        let one = ranged(1, &[(100, 1.0), (5000, 0.3)], &[(0, 1), (0, 0)]);
        assert_eq!(bake_at(&one, 1, (1000, 2000)), None);
    }

    #[test]
    fn empty_band_uses_the_window_low_key_not_the_nearest() {
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1), (0, 1)]);
        let a = bake_at(&t, 1, (23333, 26000));
        assert_eq!(a, None, "the batch stays visible (constant 1 = identity)");
    }

    #[test]
    fn missing_ranges_search_the_whole_key_list() {
        let t = track(0xffff, 0, &[(0, 1.0), (10_000, 0.0)]);
        let a = bake_at(&t, 7, (20_000, 21_000)).expect("holds 0");
        assert_eq!(a.sample(0.5), 0.0, "past the last key: hold it");
    }

    #[test]
    fn a_clamped_band_holds_its_faded_tail() {
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1)]);
        let a = bake_at_clamped(&t, 0, (43333, 46333)).expect("the band moves, so it bakes");
        assert!(!a.wrap, "a one-shot band clamps its clock");
        assert_eq!(a.sample(0.0), 1.0, "the body is still there as death opens");
        assert_eq!(
            a.sample(2.999),
            0.0,
            "…and gone by the end of the animation"
        );
        assert_eq!(a.sample(3.0), 0.0, "t == period must not alias to the head");
        assert_eq!(a.sample(3.008), 0.0, "just past the end");
        assert_eq!(a.sample(600.0), 0.0, "still gone a corpse-decay later");
    }

    #[test]
    fn a_looping_band_still_wraps() {
        let t = ranged(0, &[(3333, 1.0), (44200, 0.0)], &[(0, 1)]);
        let a = bake_at(&t, 0, (43333, 46333)).expect("the band moves, so it bakes");
        assert!(a.wrap, "a looping band wraps its clock");
        assert_eq!(a.sample(2.999), 0.0, "first pass: faded out");
        assert_eq!(a.sample(3.0), 1.0, "second pass: the window re-fires");
    }

    #[test]
    fn step_tracks_hold_between_keys() {
        let a = bake_scalar_anim(&track(0, 0, &[(0, 0.2), (1000, 1.0)]), &[2000], None)
            .expect("animates");
        assert!(a.step);
        assert_eq!(a.sample(0.999), 0.2);
        assert_eq!(a.sample(1.0), 1.0);
    }

    fn dim(v: f32) -> ScalarAnim {
        ScalarAnim {
            period: 0.0,
            step: false,
            wrap: true,
            gseq: false,
            keys: vec![(0.0, v)],
        }
    }

    #[test]
    fn alpha_anim_multiplies_color_and_weight() {
        let both = AlphaAnim::new(vec![AlphaSeq {
            color: Some(dim(0.5)),
            weight: Some(dim(0.5)),
        }])
        .expect("a dimming pair is worth carrying");
        assert!((both.sample(None, 7.0, 0.0) - 0.25).abs() < 1e-6);
        assert_eq!(AlphaAnim::new(vec![AlphaSeq::default()]), None);
    }

    #[test]
    fn alpha_anim_addresses_by_sequence_slot() {
        let a = AlphaAnim::new(vec![
            AlphaSeq {
                color: None,
                weight: Some(dim(0.0)),
            },
            AlphaSeq::default(),
            AlphaSeq {
                color: None,
                weight: Some(dim(0.5)),
            },
        ])
        .expect("animates");
        assert_eq!(a.sample(Some(0), 0.0, 0.0), 0.0);
        assert_eq!(a.sample(Some(1), 0.0, 0.0), 1.0);
        assert_eq!(a.sample(Some(2), 0.0, 0.0), 0.5);
        assert_eq!(a.sample(Some(99), 0.0, 0.0), 0.0, "out of range ⇒ slot 0");
        assert_eq!(a.sample(None, 0.0, 0.0), 0.0, "unknown sequence ⇒ slot 0");
        assert!(a.ever_hides());
        let never = AlphaAnim::new(vec![AlphaSeq {
            color: None,
            weight: Some(dim(0.5)),
        }])
        .expect("animates");
        assert!(!never.ever_hides());
    }
}
