//! The wade foam's arithmetic: what one emission looks like, how a record grows and fades, and
//! how its texture lies on the water.

use super::super::drift::rand01;

/// Seconds between ring pulses, drawn uniformly.
pub(super) const RING_INTERVAL: (f32, f32) = (0.4, 0.45);

/// Seconds until the next wake at `speed` yards a second: one wake per ~0.625 yards travelled.
pub(super) fn wake_cooldown(speed: f32, rng: &mut u32) -> f32 {
    let k = 0.9 + 0.2 * rand01(rng);
    k * 0.625 / speed.clamp(0.1, 20.0)
}

/// What a wading body is doing this frame.
#[derive(Clone, Copy)]
pub(super) enum WadeState {
    Translating { speed: f32, heading: f32 },
    Turning,
    Standing,
}

/// One emission: its first size, growth in yards a second, life in seconds, peak alpha, and
/// whether it is a ring rather than a wake.
pub(super) struct FoamParams {
    pub(super) size0: f32,
    pub(super) growth: f32,
    pub(super) lifetime: f32,
    pub(super) peak: f32,
    pub(super) ring: bool,
}

/// `gate` is the deepest a body still foams at, `depth` the water over its feet. `None` out of
/// the water or past the gate. Standing makes a smaller, slower, fainter ring; past half the gate
/// everything but the growth fades toward half.
pub(super) fn foam_params(
    state: WadeState,
    oneshot: bool,
    scale: f32,
    gate: f32,
    depth: f32,
    rng: &mut u32,
) -> Option<FoamParams> {
    if depth <= 0.0 || depth >= gate {
        return None;
    }
    let mut uni = |a: f32, b: f32| a + (b - a) * rand01(rng);
    let mut size0 = (scale * (1.0 / 3.0) * uni(0.9, 1.1)).clamp(1.0 / 3.0, 5.0 / 3.0);
    let mut lifetime = uni(0.6, 0.7);
    let mut growth = uni(1.0, 1.5);
    let mut alpha = 1.0 / 6.0;
    let ring = oneshot || !matches!(state, WadeState::Translating { .. });
    match (oneshot, state) {
        (false, WadeState::Translating { speed, .. }) => {
            growth *= speed.min(20.0) / 2.5;
        }
        (false, WadeState::Standing) => {
            alpha *= 0.8;
            growth *= 0.25;
            size0 *= 0.6;
        }
        _ => {}
    }
    let half = gate * 0.5;
    if depth > half {
        let k = 0.5 + 0.5 * (gate - depth) / half;
        alpha *= k;
        lifetime *= k;
        size0 *= k;
    }
    Some(FoamParams {
        size0,
        growth,
        lifetime,
        peak: (6.0 * alpha).min(1.0),
        ring,
    })
}

/// A record's size at `now`: it grows linearly from its first size.
pub(super) fn record_size(size0: f32, growth: f32, born: f32, now: f32) -> f32 {
    size0 + growth * (now - born)
}

/// A record's alpha at `now`: up to its peak over the first 0.4 of its life, down to nothing
/// over the rest.
pub(super) fn record_alpha(peak: f32, lifetime: f32, born: f32, now: f32) -> f32 {
    let age = (now - born) / lifetime;
    if age <= 0.4 {
        peak * (age / 0.4).max(0.0)
    } else {
        peak * (1.0 - (age - 0.4) / 0.6).max(0.0)
    }
}

/// The texture coordinate at WoW XY `p` of a record centred at `center`, `size` in each
/// direction: across the heading in `u`, against it in `v`, so the wake's apex leads.
pub(super) fn foam_uv(center: [f32; 2], heading: f32, size: f32, p: [f32; 2]) -> [f32; 2] {
    let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
    let (s, c) = heading.sin_cos();
    let inv = 1.0 / (2.0 * size);
    [
        (-s * dx + c * dy) * inv + 0.5,
        (-c * dx - s * dy) * inv + 0.5,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wake_leads_with_its_apex_and_spans_its_box() {
        let c = [-9016.0_f32, -226.0];
        let (h, s) = (0.4637_f32, 1.25_f32);
        let (sh, ch) = h.sin_cos();
        let ahead = foam_uv(c, h, s, [c[0] + ch, c[1] + sh]);
        let behind = foam_uv(c, h, s, [c[0] - ch, c[1] - sh]);
        let across = foam_uv(c, h, s, [c[0] - sh, c[1] + ch]);
        assert!((ahead[0] - 0.5).abs() < 1e-4 && ahead[1] < 0.5, "{ahead:?}");
        assert!(behind[1] > 0.5, "{behind:?}");
        assert!((across[1] - 0.5).abs() < 1e-4 && (across[0] - 0.5).abs() > 0.1);
        let edge = foam_uv(c, h, s, [c[0] + ch * s, c[1] + sh * s]);
        assert!(edge[1].abs() < 1e-4, "{edge:?}");
    }

    #[test]
    fn an_emission_takes_its_kind() {
        let mut rng = 1u32;
        let running = WadeState::Translating {
            speed: 7.0,
            heading: 0.0,
        };
        for _ in 0..64 {
            let p = foam_params(running, false, 1.0, 1.0, 0.3, &mut rng).expect("in the water");
            assert!(!p.ring);
            assert!((0.3..=0.37).contains(&p.size0), "{}", p.size0);
            assert!((2.8..=4.2).contains(&p.growth), "{}", p.growth);
            assert!((0.6..0.7).contains(&p.lifetime));
            assert!((p.peak - 1.0).abs() < 1e-6);
            let p = foam_params(WadeState::Standing, false, 1.0, 1.0, 0.3, &mut rng)
                .expect("in the water");
            assert!(p.ring);
            assert!((0.16..=0.23).contains(&p.size0), "{}", p.size0);
            assert!((0.25..=0.375).contains(&p.growth), "{}", p.growth);
            assert!((p.peak - 0.8).abs() < 1e-6);
            let p = foam_params(running, true, 1.0, 1.0, 0.3, &mut rng).expect("in the water");
            assert!(p.ring && (1.0..1.5).contains(&p.growth));
        }
    }

    #[test]
    fn a_wake_lies_every_five_eighths_of_a_yard() {
        let mut rng = 3u32;
        for _ in 0..32 {
            assert!((0.080..=0.103).contains(&wake_cooldown(7.0, &mut rng)));
            assert!((0.028..=0.036).contains(&wake_cooldown(50.0, &mut rng)));
        }
    }

    #[test]
    fn foam_fades_toward_half_past_half_the_gate() {
        let mut rng = 7u32;
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, 1.05, &mut rng).is_none());
        assert!(foam_params(WadeState::Standing, false, 1.0, 1.0, -0.1, &mut rng).is_none());
        let deep = foam_params(WadeState::Standing, false, 1.0, 1.0, 0.99, &mut rng)
            .expect("inside the gate");
        assert!((deep.peak - 0.8 * 0.505).abs() < 0.02, "{}", deep.peak);
    }

    #[test]
    fn a_record_rises_to_its_peak_and_fades_by_its_end() {
        assert!((record_alpha(0.8, 1.0, 0.0, 0.2) - 0.4).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.4) - 0.8).abs() < 1e-6);
        assert!((record_alpha(0.8, 1.0, 0.0, 0.7) - 0.4).abs() < 1e-6);
        assert!(record_alpha(0.8, 1.0, 0.0, 1.0) < 1e-6);
        assert!((record_size(0.5, 1.0, 0.0, 0.5) - 1.0).abs() < 1e-6);
    }
}
