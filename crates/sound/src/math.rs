//! The client's own gain arithmetic. It fed FMOD 0..255 levels; the mixer here takes floats, so
//! the linear value is used before the client quantised it.

use bevy::math::Vec3;

/// The client's one global rolloff factor.
pub const ROLLOFF_FACTOR: f32 = 4.0;

/// A shot's volume: `(draw − 15) · 0.01 + base` with a draw, `base` without, scaled by `mult` and
/// clamped to `[0, 1]`, NaN landing on 0.
pub fn variation_volume(draw: Option<i32>, base: f32, mult: f32) -> f32 {
    let v = match draw {
        None => f64::from(base),
        Some(r) => f64::from(r - 0xf) * 0.01 + f64::from(base),
    };
    let v = v * f64::from(mult);
    #[allow(clippy::neg_cmp_op_on_partial_ord, reason = "NaN takes the zero arm")]
    if !(v > 0.0) {
        0.0
    } else if v >= 1.0 {
        1.0
    } else {
        v as f32
    }
}

/// A shot's playback frequency in Hz: the client's integer `22050 · (draw + 85) / 100`, absolute,
/// so a 44.1 kHz file plays at half speed, as in the client.
pub fn variation_pitch_freq(draw: i32) -> i32 {
    22050i32.wrapping_mul(draw.wrapping_add(0x55)) / 100
}

/// A raw draw scaled to `0..=30` by a high multiply, not a modulo.
pub fn variation_draw(raw: u32) -> i32 {
    ((u64::from(raw) * 31) >> 32) as i32
}

pub fn dist_sq(a: Vec3, b: Vec3) -> f32 {
    a.distance_squared(b)
}

/// Audible strictly inside `maxdist`; at it, or NaN, not.
pub fn audible(d_sq: f32, maxdist: f32) -> bool {
    maxdist * maxdist > d_sq
}

/// The last tenth of `maxdist` ramps linearly to silence.
pub fn near_field_atten(d_sq: f32, maxdist: f32) -> f32 {
    let band = maxdist * 0.1;
    if band <= 0.0 {
        return 1.0;
    }
    let over = d_sq.sqrt() - maxdist * 0.9;
    let clamped = over.clamp(0.0, band);
    1.0 - clamped / band
}

/// FMOD's inverse rolloff past `min_dist` at the client's factor of 4; `0` is non-positional.
pub fn fmod_rolloff(d_sq: f32, min_dist: f32) -> f32 {
    if min_dist <= 0.0 {
        return 1.0;
    }
    let d = d_sq.sqrt();
    if d <= min_dist {
        1.0
    } else {
        min_dist / (min_dist + ROLLOFF_FACTOR * (d - min_dist))
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn volume_varies_fifteen_hundredths_around_its_base() {
        assert_eq!(variation_volume(Some(15), 0.8, 1.0), 0.8);
        assert!((variation_volume(Some(30), 0.8, 1.0) - 0.95).abs() < 1e-6);
        assert!((variation_volume(Some(0), 0.8, 1.0) - 0.65).abs() < 1e-6);
        assert_eq!(variation_volume(Some(30), 0.95, 1.0), 1.0);
        assert_eq!(variation_volume(Some(15), 0.8, 0.0), 0.0);
        assert_eq!(variation_volume(None, 0.5, 1.0), 0.5);
        assert_eq!(variation_volume(None, f32::NAN, 1.0), 0.0);
    }

    #[test]
    fn pitch_is_integer_hz() {
        assert_eq!(variation_pitch_freq(15), 22050);
        assert_eq!(variation_pitch_freq(0), 18742);
        assert_eq!(variation_pitch_freq(30), 25357);
    }

    #[test]
    fn the_draw_is_a_high_multiply() {
        assert_eq!(variation_draw(0), 0);
        assert_eq!(variation_draw(u32::MAX), 30);
        assert_eq!(variation_draw(1 << 31), 15);
    }

    #[test]
    fn distance_gates() {
        let md = 100.0;
        assert_eq!(near_field_atten(90.0 * 90.0, md), 1.0);
        assert!((near_field_atten(95.0 * 95.0, md) - 0.5).abs() < 1e-5);
        assert!(near_field_atten(100.0 * 100.0, md).abs() < 1e-5);
        assert!(audible(99.9 * 99.9, 100.0));
        assert!(!audible(100.0 * 100.0, 100.0));
        assert_eq!(fmod_rolloff(8.0 * 8.0, 8.0), 1.0);
        assert!((fmod_rolloff(16.0 * 16.0, 8.0) - 0.2).abs() < 1e-6);
        assert_eq!(fmod_rolloff(100.0, 0.0), 1.0);
    }
}
