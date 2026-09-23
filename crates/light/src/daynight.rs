//! The client's day curves: sun and moon directions, disc sizes, and fades keyed to the hour.
//!
//! Times are game minutes of the day, `0..1440`. Directions are world coordinates, Z up.

/// The direction the lighting sun shines. It never sets: its azimuth is fixed and its elevation
/// stays between 20 and 37 degrees; night comes from the colours instead.
pub fn sun_direction(minute: f32) -> [f32; 3] {
    const PHI: [(f32, f32); 4] = [
        (0.0, 2.216_568_2),
        (0.25, 1.919_862_3),
        (0.5, 2.216_568_2),
        (0.75, 1.919_862_3),
    ];
    const THETA: f32 = 3.926_991;
    spherical(THETA, interp(&PHI, minute / 1440.0))
}

/// The direction to the visible sun disc, which rises and sets: 5 degrees from the zenith at noon,
/// 10 below the horizon at night, on the lighting sun's bearing.
pub fn celestial_sun_direction(minute: f32) -> [f32; 3] {
    const PHI: [(f32, f32); 5] = [
        (0.229_166_7, 1.745_329_3),
        (0.496_527_8, 0.087_266_5),
        (0.5, 0.087_266_5),
        (0.503_472_2, 0.087_266_5),
        (0.895_833_3, 1.745_329_3),
    ];
    spherical(std::f32::consts::FRAC_PI_4, interp(&PHI, minute / 1440.0))
}

/// The direction to the white moon: 55 degrees up at midnight, 10 below the horizon from 04:00 to
/// 22:00, on the sun's bearing.
pub fn moon_direction(minute: f32) -> [f32; 3] {
    spherical(
        std::f32::consts::FRAC_PI_4,
        interp(&MOON_PHI, minute / 1440.0),
    )
}

const MOON_PHI: [(f32, f32); 5] = [
    (0.000_000, 0.610_865_2),
    (0.003_472, 0.610_865_2),
    (0.166_667, 1.745_329_3),
    (0.916_667, 1.745_329_3),
    (0.996_528, 0.610_865_2),
];

/// The second moon's direction and size multiplier. The client draws it every frame but never
/// colours it, so it renders black.
///
/// Its clock is `day_continuous` (days, fractional) modulo 1.7, and past 1.0 its tracks hold
/// their end values.
pub fn moon02_state(day_continuous: f64) -> ([f32; 3], f32) {
    const THETA: [(f32, f32); 3] = [
        (0.000_000, 2.356_194_5),
        (0.166_667, 2.617_993_8),
        (0.916_667, 2.879_793_2),
    ];
    let phase = (day_continuous.rem_euclid(1.7) as f32).min(1.0);
    (
        spherical(interp(&THETA, phase), interp(&MOON_PHI, phase)),
        interp(&MOON_SIZE, phase),
    )
}

/// The dawn and dusk highlight on the sky dome, in a zone that flags it: rising from 03:00 to a
/// peak at 06:30 and gone by 07:00, then rising from 20:30 to a peak at 21:30 and gone by 23:59.
pub fn sky_warp(minute: f32, highlight_sky: f32) -> f32 {
    const CURVE: [(f32, f32); 6] = [
        (0.1250, 0.0),
        (0.2708, 1.0),
        (0.2917, 0.0),
        (0.8542, 0.0),
        (0.8958, 1.0),
        (0.9993, 0.0),
    ];
    interp(&CURVE, minute / 1440.0) * highlight_sky
}

/// The sun disc's size multiplier: 2 at the horizon at 06:00 and 21:00, 1 through the day.
pub fn sun_disc_scale(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 4] = [(0.25, 2.0), (0.281_25, 1.0), (0.843_75, 1.0), (0.875, 2.0)];
    interp(&CURVE, minute / 1440.0)
}

const MOON_SIZE: [(f32, f32); 4] = [
    (0.041_667, 1.0),
    (0.166_667, 1.5),
    (0.916_667, 1.5),
    (0.999_306, 1.0),
];

/// The white moon's size multiplier: 1.5 at 04:00 and 22:00, 1 around midnight.
pub fn moon_disc_scale(minute: f32) -> f32 {
    interp(&MOON_SIZE, minute / 1440.0)
}

/// The star field's alpha: full from 00:00 to 03:00, out by 04:30, fading back in from 22:30.
pub fn star_alpha(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 4] = [(0.0, 1.0), (0.125, 1.0), (0.1875, 0.0), (0.9375, 0.0)];
    interp(&CURVE, minute / 1440.0)
}

/// The sun flare's envelope: off until 06:30, full from 07:30 to 19:30, off by 21:00.
pub fn sun_flare_dn(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 4] = [
        (0.270_833_3, 0.0),
        (0.3125, 1.0),
        (0.8125, 1.0),
        (0.875, 0.0),
    ];
    interp(&CURVE, minute / 1440.0)
}

/// The moon flare's envelope: off from 03:15 to 22:45, full from 23:59 to 02:00.
pub fn moon_flare_dn(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 4] = [
        (0.083_333_3, 1.0),
        (0.135_416_7, 0.0),
        (0.947_916_7, 0.0),
        (0.999_306, 1.0),
    ];
    interp(&CURVE, minute / 1440.0)
}

/// How brightly night-lit WMO materials glow: 1 from 21:30 to 06:00, 0 from 07:00 to 20:30.
pub fn sidn_night_fraction(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 4] = [
        (0.25, 1.0),
        (0.291_666_7, 0.0),
        (0.854_166_7, 0.0),
        (0.895_833_3, 1.0),
    ];
    interp(&CURVE, minute / 1440.0)
}

/// The clouds' sun-glow envelope: 1 through the day, notching to 0 at dawn and at 22:10.
///
/// The client stores the last two keys out of time order, so the scan never stops at them: past
/// 22:10 the curve wraps from the last key and snaps back to 1.
pub fn cloud_glow_track(minute: f32) -> f32 {
    const CURVE: [(f32, f32); 8] = [
        (0.166_67, 1.0),
        (0.194_44, 0.0),
        (0.201_39, 0.0),
        (0.229_17, 1.0),
        (0.895_83, 1.0),
        (0.923_61, 0.0),
        (0.888_89, 0.0),
        (0.916_67, 1.0),
    ];
    interp(&CURVE, minute / 1440.0)
}

/// Whether the sun, rather than the moon, lights the clouds: from 04:50 to 22:10.
pub fn cloud_glow_is_sun(minute: f32) -> bool {
    let day_fraction = minute / 1440.0;
    (0.201_388_9..=0.923_611_1).contains(&day_fraction)
}

fn spherical(theta: f32, phi: f32) -> [f32; 3] {
    let (sp, cp) = (phi.sin(), phi.cos());
    [sp * theta.cos(), sp * theta.sin(), cp]
}

/// The client's day-table lookup: keys scanned in stored order, never sorted.
fn interp(table: &[(f32, f32)], day_fraction: f32) -> f32 {
    let n = table.len();
    let mut a = 0;
    while a < n && day_fraction > table[a].0 {
        a += 1;
    }
    let (a, b) = if a == n || a == 0 {
        (if a == n { 0 } else { a }, n - 1)
    } else {
        (a, a - 1)
    };
    let mut span = table[a].0 - table[b].0;
    if span < 0.0 {
        span += 1.0;
    }
    let mut into = day_fraction - table[b].0;
    if into < 0.0 {
        into += 1.0;
    }
    let t = if span == 0.0 { 0.0 } else { into / span };
    table[b].1 + t * (table[a].1 - table[b].1)
}

#[cfg(test)]
mod tests;
