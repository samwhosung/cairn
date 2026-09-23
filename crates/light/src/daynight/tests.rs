use super::*;

fn elevation_deg(d: [f32; 3]) -> f32 {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    (d[2] / len).asin().to_degrees()
}

fn heading(d: [f32; 3]) -> f32 {
    d[1].atan2(d[0])
}

fn bits(x: f32) -> u32 {
    x.to_bits()
}

#[test]
fn cloud_glow_follows_the_stored_keys() {
    assert_eq!(bits(cloud_glow_track(720.0)), bits(1.0));
    assert_eq!(bits(cloud_glow_track(0.20139 * 1440.0)), bits(0.0));
    assert!(cloud_glow_track(0.9236 * 1440.0) < 0.01);
    assert_eq!(bits(cloud_glow_track(0.9237 * 1440.0)), bits(1.0));
    assert_eq!(bits(cloud_glow_track(0.95 * 1440.0)), bits(1.0));
    assert!((cloud_glow_track(0.9097 * 1440.0) - 0.5).abs() < 0.01);
    assert!(cloud_glow_is_sun(720.0));
    assert!(!cloud_glow_is_sun(0.95 * 1440.0));
}

#[test]
fn interpolation_hits_the_keys_and_halves_between_them() {
    const PHI: [(f32, f32); 4] = [
        (0.0, 2.216_568_2),
        (0.25, 1.919_862_3),
        (0.5, 2.216_568_2),
        (0.75, 1.919_862_3),
    ];
    assert!((interp(&PHI, 0.0) - 2.216_568_2).abs() < 1e-5);
    assert!((interp(&PHI, 0.25) - 1.919_862_3).abs() < 1e-5);
    assert!((interp(&PHI, 0.5) - 2.216_568_2).abs() < 1e-5);
    assert!((interp(&PHI, 0.125) - 2.068_215_3).abs() < 1e-4);
}

#[test]
fn night_glow_ramps_at_dusk_and_dawn() {
    let f = |h: f32, m: f32| sidn_night_fraction(h * 60.0 + m);
    assert!((f(0.0, 0.0) - 1.0).abs() < 1e-5);
    assert!((f(6.0, 0.0) - 1.0).abs() < 1e-5);
    assert!((f(6.0, 30.0) - 0.5).abs() < 1e-5);
    assert!(f(7.0, 0.0).abs() < 1e-5);
    assert!(f(12.0, 0.0).abs() < 1e-5);
    assert!(f(20.0, 30.0).abs() < 1e-5);
    assert!((f(21.0, 0.0) - 0.5).abs() < 1e-5);
    assert!((f(21.0, 30.0) - 1.0).abs() < 1e-4);
    assert!((f(23.0, 0.0) - 1.0).abs() < 1e-5);
}

#[test]
fn the_lighting_sun_never_sets_or_turns() {
    let noon = heading(sun_direction(720.0));
    for minute in (0..1440).step_by(30) {
        let d = sun_direction(minute as f32);
        assert!(d[2] < 0.0, "minute {minute}");
        assert!((heading(d) - noon).abs() < 1e-4, "minute {minute}");
        let up = -elevation_deg(d);
        assert!((19.9..37.1).contains(&up), "minute {minute}: {up}");
    }
}

#[test]
fn the_visible_sun_rises_and_sets() {
    let noon = elevation_deg(celestial_sun_direction(720.0));
    let midnight = elevation_deg(celestial_sun_direction(0.0));
    let morning = elevation_deg(celestial_sun_direction(480.0));
    assert!((noon - 85.0).abs() < 1.0, "{noon}");
    assert!((midnight + 10.0).abs() < 1.0, "{midnight}");
    assert!((morning - 27.0).abs() < 2.0, "{morning}");
    assert!(celestial_sun_direction(1380.0)[2] < 0.0);
}

#[test]
fn both_suns_share_a_bearing() {
    let light_to_sun = sun_direction(600.0).map(|v| -v);
    let disc = celestial_sun_direction(600.0);
    assert!((heading(disc) - heading(light_to_sun)).abs() < 1e-3);
    assert!(elevation_deg(disc) > elevation_deg(light_to_sun) + 15.0);
}

#[test]
fn the_sky_warp_spikes_at_dawn_and_dusk_only() {
    let s = |min: u32| sky_warp(min as f32, 1.0);
    assert_eq!(bits(s(720)), bits(0.0));
    assert_eq!(bits(s(0)), bits(0.0));
    assert_eq!(bits(s(1080)), bits(0.0));
    assert!(s(390) > 0.99);
    assert!(s(1290) > 0.99);
    assert!(s(360) > 0.0 && s(360) < 1.0);
    assert_eq!(bits(sky_warp(390.0, 0.0)), bits(0.0));
}

#[test]
fn discs_grow_at_the_horizon() {
    assert!((sun_disc_scale(360.0) - 2.0).abs() < 1e-3);
    assert!((sun_disc_scale(1260.0) - 2.0).abs() < 1e-3);
    assert!((sun_disc_scale(720.0) - 1.0).abs() < 1e-3);
    let r = sun_disc_scale(380.0);
    assert!(r > 1.0 && r < 2.0);
    assert!((moon_disc_scale(1320.0) - 1.5).abs() < 1e-3);
    assert!((moon_disc_scale(240.0) - 1.5).abs() < 1e-3);
    assert!((moon_disc_scale(60.0) - 1.0).abs() < 1e-3);
}

#[test]
fn stars_are_out_only_at_night() {
    assert!((star_alpha(0.0) - 1.0).abs() < 1e-3);
    assert!((star_alpha(120.0) - 1.0).abs() < 1e-3);
    assert_eq!(bits(star_alpha(720.0)), bits(0.0));
    assert_eq!(bits(star_alpha(1080.0)), bits(0.0));
    let dusk = star_alpha(1395.0);
    assert!(dusk > 0.0 && dusk < 1.0);
    let dawn = star_alpha(225.0);
    assert!(dawn > 0.0 && dawn < 1.0);
}

#[test]
fn flares_follow_their_envelopes() {
    assert_eq!(bits(moon_flare_dn(1350.0)), bits(0.0));
    assert_eq!(bits(moon_flare_dn(720.0)), bits(0.0));
    assert!((moon_flare_dn(1380.0) - 0.203).abs() < 5e-3);
    assert!((moon_flare_dn(1410.0) - 0.608).abs() < 5e-3);
    assert!((moon_flare_dn(60.0) - 1.0).abs() < 1e-3);
    assert!(moon_flare_dn(195.0) < 1e-3);
    assert!((sun_flare_dn(720.0) - 1.0).abs() < 1e-3);
    assert_eq!(bits(sun_flare_dn(1260.0)), bits(0.0));
    assert_eq!(bits(sun_flare_dn(1380.0)), bits(0.0));
    assert!(sun_flare_dn(390.0) < 1e-3);
}

#[test]
fn the_second_moon_drifts_and_parks() {
    let (dir, scale) = moon02_state(0.0);
    assert!(dir[2] > 0.5);
    assert!((scale - 1.0).abs() < 1e-3);
    assert!(moon02_state(0.5).0[2] < 0.0);
    let (a, sa) = moon02_state(1.05);
    let (b, sb) = moon02_state(1.65);
    assert!(a[2] > 0.5);
    assert_eq!(a.map(bits), b.map(bits));
    assert_eq!(bits(sa), bits(sb));
    assert!(moon02_state(10.0).0[2] > 0.5);
    assert!(moon02_state(11.0).0[2] < 0.0);
    let apart = (heading(moon02_state(0.0).0) - heading(moon_direction(0.0))).to_degrees();
    assert!(apart.abs() > 30.0);
}

#[test]
fn the_white_moon_is_up_at_night() {
    assert!(moon_direction(0.0)[2] > 0.5);
    assert!(moon_direction(720.0)[2] < 0.0);
}
