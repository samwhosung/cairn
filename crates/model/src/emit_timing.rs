use m2::M2ScalarTrack;

use crate::key_anim::{SeqSlot, bake_track};
use crate::mat_anim::ScalarAnim;

fn bake_per_slot(
    track: &M2ScalarTrack,
    slots: &[SeqSlot],
    gseq: &[u32],
) -> Vec<Option<ScalarAnim>> {
    if track.keys.is_empty() {
        return Vec::new();
    }
    slots
        .iter()
        .map(|&s| bake_track(track, gseq, Some(s), |v| v, |_| false, |_| false))
        .collect()
}

/// A particle emitter's spawn rate and on/off gate, baked one loop per file sequence slot: the
/// client samples both every frame through the playing sequence's window, and a looping
/// sequence wraps its band while a one-shot one holds its tail.
#[derive(Debug, Clone, Default)]
pub struct EmitTiming {
    rate: Vec<Option<ScalarAnim>>,
    enabled: Vec<Option<ScalarAnim>>,
    looping: Vec<bool>,
}

impl EmitTiming {
    pub(crate) fn bake(
        rate: &M2ScalarTrack,
        enabled: &M2ScalarTrack,
        slots: &[SeqSlot],
        gseq: &[u32],
    ) -> Self {
        let per_slot = |t: &M2ScalarTrack| bake_per_slot(t, slots, gseq);
        Self {
            rate: per_slot(rate),
            enabled: per_slot(enabled),
            looping: slots.iter().map(|s| s.looping).collect(),
        }
    }

    fn idx(&self, seq: Option<usize>) -> usize {
        match seq {
            Some(i) if i < self.looping.len() => i,
            _ => 0,
        }
    }

    /// Whether the gate is on `elapsed` seconds into sequence slot `seq` (slot 0 when `seq` is
    /// `None` or out of range); a slot without a gate is on, as the client loads it. `shared_now`
    /// is the global-sequence clock ([`crate::KeyAnim::clock`]).
    pub fn emitting(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> bool {
        self.enabled
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .is_none_or(|a| a.sample_or(a.clock(elapsed, shared_now), 1.0) > 0.5)
    }

    /// Particles a second, `elapsed` seconds into slot `seq`, never below 0; 0 when the track
    /// has no keys.
    pub fn rate(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> f32 {
        self.rate
            .get(self.idx(seq))
            .and_then(|o| o.as_ref())
            .map_or(0.0, |a| a.sample_or(a.clock(elapsed, shared_now), 0.0))
            .max(0.0)
    }

    /// The largest rate key in any slot.
    pub fn peak_rate(&self) -> f32 {
        self.rate
            .iter()
            .flatten()
            .flat_map(|a| a.keys.iter().map(|&(_, v)| v))
            .fold(0.0, f32::max)
    }

    /// A burst emitter's first burst in slot `seq`, as `(seconds, count)`: the first 60 Hz frame
    /// of the slot's keyed span where the gate is on and the rate above 0, the count being the
    /// rate truncated.
    pub fn first_burst(&self, seq: Option<usize>) -> Option<(f32, f32)> {
        const STEP: f32 = 1.0 / 60.0;
        let i = self.idx(seq);
        let span = |slots: &[Option<ScalarAnim>]| -> f32 {
            slots
                .get(i)
                .and_then(Option::as_ref)
                .and_then(|a| a.keys.last())
                .map_or(0.0, |&(t, _)| t)
        };
        let end = span(&self.rate).max(span(&self.enabled));
        let mut frame = 0;
        loop {
            let t = frame as f32 * STEP;
            if t > end + STEP {
                return None;
            }
            let rate = self.rate(seq, t, 0.0);
            if rate > 0.0 && self.emitting(seq, t, 0.0) {
                return Some((t, rate.trunc()));
            }
            frame += 1;
        }
    }

    /// The rate, when every slot bakes the same single key.
    pub fn constant_rate(&self) -> Option<f32> {
        let mut it = self.rate.iter();
        let first = it.next()?.as_ref()?;
        if first.keys.len() != 1 {
            return None;
        }
        let &(_, v) = first.keys.first()?;
        it.all(|a| a.as_ref().is_some_and(|a| a.keys == first.keys))
            .then_some(v)
    }

    /// Per file slot: whether it loops, and its rate and gate keys in seconds from its start.
    #[allow(clippy::type_complexity, reason = "a read-only view")]
    pub fn slot_views(&self) -> Vec<(bool, Option<&[(f32, f32)]>, Option<&[(f32, f32)]>)> {
        fn keys(list: &[Option<ScalarAnim>], i: usize) -> Option<&[(f32, f32)]> {
            list.get(i)
                .and_then(|o| o.as_ref())
                .map(|a| a.keys.as_slice())
        }
        (0..self.looping.len())
            .map(|i| (self.looping[i], keys(&self.rate, i), keys(&self.enabled, i)))
            .collect()
    }

    /// One always-on looping slot at a constant `rate`.
    pub fn constant(rate: f32) -> Self {
        Self {
            rate: vec![Some(ScalarAnim {
                period: 0.0,
                step: true,
                wrap: true,
                gseq: false,
                keys: vec![(0.0, rate)],
            })],
            enabled: vec![None],
            looping: vec![true],
        }
    }
}

/// An emitter's nine per-frame parameters: as values ([`ParamsNow`]), as the record's tracks, or
/// as those tracks baked per slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params<T> {
    /// Yards a second.
    pub emission_speed: T,
    /// The speed's spread: `speed · (1 ± variation)`.
    pub speed_variation: T,
    /// Radians: the cone's half-angle; a sphere's latitude range; a spline's spin about its
    /// tangent.
    pub vertical_range: T,
    /// Radians: the azimuth spread; a sphere's longitude range; a spline's scatter, in yards.
    pub horizontal_range: T,
    /// Yards a second squared, down.
    pub gravity: T,
    /// Seconds; a particle keeps the value it was born with.
    pub lifespan: T,
    /// A plane's full length, along y once the client's quarter turn is applied; a sphere's
    /// inner radius; a spline's first arc fraction.
    pub area_length: T,
    /// A plane's full width, along x once turned; a sphere's outer radius; a spline's last arc
    /// fraction.
    pub area_width: T,
    /// Births fly away from `(0, 0, z_source)` when it is not 0.
    pub z_source: T,
}

impl<T> Params<T> {
    fn map<U>(self, mut f: impl FnMut(T) -> U) -> Params<U> {
        Params {
            emission_speed: f(self.emission_speed),
            speed_variation: f(self.speed_variation),
            vertical_range: f(self.vertical_range),
            horizontal_range: f(self.horizontal_range),
            gravity: f(self.gravity),
            lifespan: f(self.lifespan),
            area_length: f(self.area_length),
            area_width: f(self.area_width),
            z_source: f(self.z_source),
        }
    }

    fn zip<U>(self, other: Params<U>) -> Params<(T, U)> {
        Params {
            emission_speed: (self.emission_speed, other.emission_speed),
            speed_variation: (self.speed_variation, other.speed_variation),
            vertical_range: (self.vertical_range, other.vertical_range),
            horizontal_range: (self.horizontal_range, other.horizontal_range),
            gravity: (self.gravity, other.gravity),
            lifespan: (self.lifespan, other.lifespan),
            area_length: (self.area_length, other.area_length),
            area_width: (self.area_width, other.area_width),
            z_source: (self.z_source, other.z_source),
        }
    }

    fn as_ref(&self) -> Params<&T> {
        Params {
            emission_speed: &self.emission_speed,
            speed_variation: &self.speed_variation,
            vertical_range: &self.vertical_range,
            horizontal_range: &self.horizontal_range,
            gravity: &self.gravity,
            lifespan: &self.lifespan,
            area_length: &self.area_length,
            area_width: &self.area_width,
            z_source: &self.z_source,
        }
    }
}

pub type ParamsNow = Params<f32>;

impl Default for ParamsNow {
    /// What keyless tracks read.
    fn default() -> Self {
        Self {
            emission_speed: 0.0,
            speed_variation: 0.0,
            vertical_range: 0.0,
            horizontal_range: 0.0,
            gravity: 0.0,
            lifespan: 1.0,
            area_length: 0.0,
            area_width: 0.0,
            z_source: 0.0,
        }
    }
}

/// [`ParamsNow`]'s tracks, baked per slot like [`EmitTiming`]'s rate.
#[derive(Debug, Clone)]
pub struct EmitParams {
    channels: Params<Vec<Option<ScalarAnim>>>,
}

impl Default for EmitParams {
    fn default() -> Self {
        Self {
            channels: ParamsNow::default().map(|_| Vec::new()),
        }
    }
}

impl EmitParams {
    pub(crate) fn bake(tracks: Params<&M2ScalarTrack>, slots: &[SeqSlot], gseq: &[u32]) -> Self {
        Self {
            channels: tracks.map(|t| bake_per_slot(t, slots, gseq)),
        }
    }

    /// Every parameter `elapsed` seconds into slot `seq`, resolved as [`EmitTiming`] does.
    pub fn sample(&self, seq: Option<usize>, elapsed: f32, shared_now: f64) -> ParamsNow {
        self.channels
            .as_ref()
            .zip(ParamsNow::default())
            .map(|(ch, default)| {
                let slot = match seq {
                    Some(s) if s < ch.len() => s,
                    _ => 0,
                };
                ch.get(slot).and_then(|o| o.as_ref()).map_or(default, |a| {
                    a.sample_or(a.clock(elapsed, shared_now), default)
                })
            })
    }

    /// The largest lifespan key in any slot, or the default when no slot keys one.
    pub fn peak_lifespan(&self) -> f32 {
        let mut keys = self
            .channels
            .lifespan
            .iter()
            .flatten()
            .flat_map(|a| a.keys.iter().map(|&(_, v)| v))
            .peekable();
        if keys.peek().is_none() {
            return ParamsNow::default().lifespan;
        }
        keys.fold(0.0, f32::max)
    }

    pub fn constant(now: ParamsNow) -> Self {
        Self {
            channels: now.map(|v| {
                vec![Some(ScalarAnim {
                    period: 0.0,
                    step: true,
                    wrap: true,
                    gseq: false,
                    keys: vec![(0.0, v)],
                })]
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn track(interp: u16, keys: &[(u32, f32)], ranges: &[(u32, u32)]) -> M2ScalarTrack {
        M2ScalarTrack {
            interp,
            gseq: 0xffff,
            ranges: ranges.to_vec(),
            keys: keys.to_vec(),
        }
    }

    fn slots(spec: &[((u32, u32), bool)]) -> Vec<SeqSlot> {
        spec.iter()
            .enumerate()
            .map(|(file_index, &(band_ms, looping))| SeqSlot {
                file_index,
                band_ms,
                looping,
            })
            .collect()
    }

    #[test]
    fn a_step_rate_is_silent_before_its_key_and_holds_after() {
        let t = EmitTiming::bake(
            &track(0, &[(0, 0.0), (67, 30.0)], &[]),
            &M2ScalarTrack::default(),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert_eq!(t.rate(None, 0.050, 0.0), 0.0);
        assert_eq!(t.rate(None, 0.070, 0.0), 30.0);
        assert_eq!(t.rate(None, 0.500, 0.0), 30.0);
        assert!(t.emitting(None, 0.5, 0.0), "no gate is always on");
    }

    #[test]
    fn a_burst_without_a_gate_fires_on_its_first_rate_key() {
        let t = EmitTiming::bake(
            &track(0, &[(0, 0.0), (67, 30.0)], &[]),
            &M2ScalarTrack::default(),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        let (at, count) = t.first_burst(Some(0)).expect("a gateless burst fires");
        assert_eq!(count, 30.0);
        assert!((0.067..0.1).contains(&at), "{at}");
    }

    #[test]
    fn a_linear_ramp_rises_and_falls_back() {
        let t = EmitTiming::bake(
            &track(1, &[(0, 0.0), (100, 100.0), (200, 0.0)], &[]),
            &M2ScalarTrack::default(),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert!((t.rate(None, 0.050, 0.0) - 50.0).abs() < 1e-4);
        assert!((t.rate(None, 0.150, 0.0) - 50.0).abs() < 1e-4);
        assert_eq!(t.rate(None, 0.500, 0.0), 0.0);
    }

    #[test]
    fn an_idle_window_is_off_and_a_one_shot_clip_holds_its_tail() {
        let gate = track(
            0,
            &[(1000, 1.0), (1333, 0.0), (3800, 1.0)],
            &[(0, 1), (1, 1), (2, 2)],
        );
        let t = EmitTiming::bake(
            &track(0, &[(0, 20.0)], &[]),
            &gate,
            &slots(&[
                ((1000, 2000), false),
                ((2333, 2667), true),
                ((3800, 4100), false),
            ]),
            &[],
        );
        assert!(!t.emitting(Some(1), 0.0, 0.0));
        assert!(!t.emitting(Some(1), 400.0, 0.0));
        assert!(t.emitting(Some(0), 0.1, 0.0));
        assert!(!t.emitting(Some(0), 0.5, 0.0));
        assert!(!t.emitting(Some(0), 1.0, 0.0), "the end does not wrap");
        assert!(!t.emitting(Some(0), 5.0, 0.0));
        assert!(t.emitting(Some(2), 0.05, 0.0));
        assert!(t.emitting(None, 0.1, 0.0));
        assert!(!t.emitting(Some(9), 0.5, 0.0));
        assert_eq!(t.constant_rate(), Some(20.0));
        assert_eq!(t.peak_rate(), 20.0);
    }

    #[test]
    fn an_animated_radius_is_sampled_where_the_clip_is() {
        let area = track(1, &[(0, 0.1944), (667, 13.1967), (867, 13.1967)], &[]);
        let life = track(1, &[(0, 0.472), (467, 0.8008), (667, 0.7), (867, 0.7)], &[]);
        let zero = M2ScalarTrack::default();
        let p = EmitParams::bake(
            Params {
                lifespan: &life,
                area_length: &area,
                area_width: &area,
                ..ParamsNow::default().map(|_| &zero)
            },
            &slots(&[((0, 867), false)]),
            &[],
        );
        let at = |t: f32| p.sample(None, t, 0.0);
        assert!((at(0.0).area_length - 0.1944).abs() < 1e-3);
        let mid = at(0.3335).area_length;
        assert!((mid - (0.1944 + (13.1967 - 0.1944) * 0.5)).abs() < 0.05);
        assert!((at(0.8).area_width - 13.1967).abs() < 1e-3);
        assert!((at(0.2).lifespan - 0.6127).abs() < 5e-3);
        assert_eq!(at(0.5).emission_speed, 0.0);
        assert!((p.peak_lifespan() - 0.8008).abs() < 1e-4);
        assert!((at(5.0).area_length - 13.1967).abs() < 1e-3);
    }

    #[test]
    fn a_looping_band_refires_its_gate() {
        let t = EmitTiming::bake(
            &track(0, &[(0, 40.0)], &[]),
            &track(0, &[(0, 1.0), (200, 0.0)], &[]),
            &slots(&[((0, 1000), true)]),
            &[],
        );
        assert!(t.emitting(None, 0.1, 0.0));
        assert!(!t.emitting(None, 0.8, 0.0));
        assert!(t.emitting(None, 1.1, 0.0));
    }

    #[test]
    fn a_burst_keyed_only_in_a_later_variation_is_silent_in_slot_0() {
        let rate = track(
            1,
            &[
                (0, 0.0),
                (1633, 0.0),
                (1667, 30.0),
                (1800, 30.0),
                (1833, 0.0),
            ],
            &[],
        );
        let t = EmitTiming::bake(
            &rate,
            &M2ScalarTrack::default(),
            &slots(&[((0, 1333), true), ((1367, 2667), true)]),
            &[],
        );
        for s in [0.0, 0.3, 0.6, 0.9, 1.2] {
            assert_eq!(t.rate(Some(0), s, 0.0), 0.0);
        }
        assert_eq!(t.rate(Some(1), 0.316, 0.0), 30.0);
        assert_eq!(t.rate(Some(1), 0.5, 0.0), 0.0);
        assert_eq!(t.peak_rate(), 30.0, "the peak folds across slots");
        assert_eq!(t.constant_rate(), None);
    }
}
