use std::path::PathBuf;
use std::sync::OnceLock;

use light::{Atmosphere, LightCatalog, Submersion, ZERO_KEY_COLOR, ZERO_KEY_SCALAR};

const EASTERN_KINGDOMS: u32 = 0;
const EMERALD_DREAM: u32 = 169;
const DEEPRUN_TRAM: u32 = 369;
const NOON: u32 = 1440;

fn catalog() -> Option<&'static LightCatalog> {
    static CATALOG: OnceLock<Option<LightCatalog>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
                eprintln!("skipped: WOW_DATA is not set");
                return None;
            };
            let chain = mpq::Chain::open(data).expect("open the chain");
            Some(LightCatalog::load(&chain).expect("load the lighting tables"))
        })
        .as_ref()
}

fn rgb(c: [f32; 3]) -> [i32; 3] {
    c.map(|v| (v * 255.0).round() as i32)
}

fn dry(cat: &LightCatalog, map: u32, pos: [f32; 3], time: u32) -> Atmosphere {
    cat.sample(map, pos, time, false, Submersion::Dry, false)
}

#[test]
fn the_nearest_sphere_at_full_weight_wins_the_blend() {
    let Some(cat) = catalog() else { return };
    let pos = [2250.0, -750.0, 50.0];
    let blended = dry(cat, EASTERN_KINGDOMS, pos, NOON);
    let picked = cat.sample_smallest_sphere(EASTERN_KINGDOMS, pos, NOON, false);
    assert_eq!(rgb(blended.ambient), rgb(picked.ambient));
    assert_eq!(rgb(blended.ambient), [80, 63, 79]);
    assert_eq!(rgb(blended.water_river[0]), [82, 64, 49]);
    assert_eq!(rgb(blended.water_river[1]), [35, 28, 37]);
}

#[test]
fn water_colour_crosses_a_sphere_edge_without_a_step() {
    let Some(cat) = catalog() else { return };
    let a = dry(cat, EASTERN_KINGDOMS, [1391.07, 641.39, 35.37], NOON);
    let b = dry(cat, EASTERN_KINGDOMS, [1401.03, 628.90, 35.42], NOON);
    for (x, y) in [
        (a.water_river[0], b.water_river[0]),
        (a.water_river[1], b.water_river[1]),
        (a.water_ocean[0], b.water_ocean[0]),
        (a.water_ocean[1], b.water_ocean[1]),
    ] {
        let (x, y) = (rgb(x), rgb(y));
        assert!((0..3).all(|i| (x[i] - y[i]).abs() <= 2), "{x:?} -> {y:?}");
    }
    assert_eq!(rgb(a.water_river[0]), [82, 93, 46]);
}

#[test]
fn stranglethorn_water_is_its_own_record() {
    let Some(cat) = catalog() else { return };
    let stv = dry(cat, EASTERN_KINGDOMS, [-13333.3, 0.0, 50.0], NOON);
    assert_eq!(rgb(stv.water_river[0]), [90, 140, 140]);
    assert_eq!(rgb(stv.water_river[1]), [28, 55, 64]);
    assert_eq!(rgb(stv.water_ocean[0]), [90, 171, 140]);
    assert_eq!(rgb(stv.water_ocean[1]), [28, 44, 44]);
    assert!((stv.water_river_alpha[0] - 216.0 / 255.0).abs() < 0.01);
}

#[test]
fn a_map_without_lights_takes_light_1() {
    let Some(cat) = catalog() else { return };
    let tram = dry(cat, DEEPRUN_TRAM, [25.0, -1256.0, -117.0], NOON);
    assert_eq!(rgb(tram.fog_color), [77, 120, 143]);
    assert_eq!(rgb(tram.ambient), [104, 130, 154]);
    assert_eq!(rgb(tram.sun_diffuse), [255, 136, 0]);
    assert!((tram.fog_end - 500.0).abs() < 1.0);
    assert!((tram.fog_start_frac - 0.25).abs() < 1e-3);
    let elsewhere = dry(cat, 4242, [9000.0, -1000.0, 60.0], NOON);
    assert_eq!(rgb(elsewhere.fog_color), rgb(tram.fog_color));
    let midnight = dry(cat, DEEPRUN_TRAM, [25.0, -1256.0, -117.0], 0);
    assert_ne!(rgb(midnight.fog_color), rgb(tram.fog_color));

    let dream = dry(cat, EMERALD_DREAM, [0.0, 0.0, 0.0], NOON);
    assert_ne!(rgb(dream.fog_color), rgb(tram.fog_color));
}

#[test]
fn keyless_rows_sample_to_zero() {
    let Some(cat) = catalog() else { return };
    let blasted_lands = cat.sample_params_id(36, NOON).expect("record 36");
    assert_eq!(rgb(blasted_lands.cloud_colors[2]), rgb(ZERO_KEY_COLOR));
    assert_eq!(rgb(blasted_lands.cloud_colors[0]), [133, 47, 0]);
    assert_eq!(rgb(blasted_lands.cloud_colors[1]), [0, 34, 88]);
    assert!((blasted_lands.cloud_density - 0.85).abs() < 1e-6);
    let underwater = cat.sample_params_id(95, NOON).expect("record 95");
    assert_eq!(underwater.fog_end.to_bits(), ZERO_KEY_SCALAR.to_bits());
    assert_eq!(
        underwater.fog_start_frac.to_bits(),
        ZERO_KEY_SCALAR.to_bits()
    );
    assert_eq!(rgb(underwater.fog_color), rgb(ZERO_KEY_COLOR));
}

#[test]
fn records_past_the_id_gaps_read_their_own_bands() {
    let Some(cat) = catalog() else { return };
    let last = cat.sample_params_id(499, NOON).expect("record 499");
    assert_eq!(rgb(last.ambient), [75, 97, 124]);
    assert_eq!(rgb(last.sun_diffuse), [255, 148, 0]);
    assert_eq!(rgb(last.fog_color), [107, 114, 136]);
    assert_eq!(rgb(last.sky[0]), [54, 56, 73]);
    assert_eq!(rgb(last.sky[4]), [117, 124, 149]);
    assert!((last.fog_end - 14000.0 / 36.0).abs() < 1e-3);
    assert!((last.fog_start_frac + 0.2).abs() < 1e-6);
    assert!(cat.sample_params_id(0, NOON).is_none());
    assert!(cat.sample_params_id(10_000, NOON).is_none());
    assert!(cat.sample_params_id(u32::MAX / 18 + 2, NOON).is_none());
}

#[test]
fn magma_slime_and_ghosts_take_their_own_profiles() {
    let Some(cat) = catalog() else { return };
    let pos = [-9000.0, 400.0, 90.0];
    let under = |s: Submersion| cat.sample(EASTERN_KINGDOMS, pos, NOON, false, s, false);
    let magma = under(Submersion::Magma);
    assert_eq!(rgb(magma.fog_color), [200, 52, 0]);
    assert!((magma.fog_end - 27.0).abs() < 1.0);
    assert!((magma.fog_start_frac + 2.0).abs() < 1e-6);
    let slime = under(Submersion::Slime);
    assert_eq!(rgb(slime.fog_color), [0, 255, 0]);
    assert!((slime.fog_end - 50.0).abs() < 1.0);
    assert!((slime.fog_start_frac + 1.0).abs() < 1e-6);
    assert_eq!(
        cat.ghost_skybox(EASTERN_KINGDOMS, pos),
        Some("environments\\stars\\deathclouds.m2")
    );
}

#[test]
fn a_light_alone_is_the_blend_where_nothing_is_over_it() {
    let Some(cat) = catalog() else { return };
    let goldshire_under_no_sphere = [-9439.1, 71.2, 68.0];
    let duskwood_deep_in_its_own = [-10640.0, -880.0, 50.0];
    let azeroth = cat
        .light_at(EASTERN_KINGDOMS, goldshire_under_no_sphere)
        .expect("a light");
    for s in [Submersion::Dry, Submersion::Water, Submersion::Magma] {
        let alone = cat.sample_light(azeroth, NOON, false, s, false);
        let blend = cat.sample(
            EASTERN_KINGDOMS,
            goldshire_under_no_sphere,
            NOON,
            false,
            s,
            false,
        );
        assert_eq!(format!("{alone:?}"), format!("{blend:?}"), "{s:?}");
    }
    let own = cat
        .light_at(EASTERN_KINGDOMS, duskwood_deep_in_its_own)
        .expect("a light");
    assert_ne!(own, azeroth);
    let dusk = cat.sample_light(own, NOON, false, Submersion::Dry, false);
    let blend = dry(cat, EASTERN_KINGDOMS, duskwood_deep_in_its_own, NOON);
    assert_eq!(rgb(dusk.fog_color), rgb(blend.fog_color));
    let noon = cat.sample_light(azeroth, NOON, false, Submersion::Dry, false);
    assert_ne!(rgb(dusk.fog_color), rgb(noon.fog_color));
    assert_eq!(
        format!(
            "{:?}",
            cat.sample_light(0, NOON, false, Submersion::Dry, false)
        ),
        format!("{:?}", Atmosphere::DEFAULT)
    );
}

#[test]
fn storms_bring_the_fog_to_the_eye() {
    let Some(cat) = catalog() else { return };
    let pos = [-9000.0, 400.0, 90.0];
    let storm = cat.sample(EASTERN_KINGDOMS, pos, NOON, true, Submersion::Dry, false);
    let clear = cat.sample(EASTERN_KINGDOMS, pos, NOON, false, Submersion::Dry, false);
    assert!(storm.fog_start_frac < 0.0, "{}", storm.fog_start_frac);
    assert!(storm.fog_end < clear.fog_end);
}

#[test]
fn reports_name_what_they_show() {
    let Some(cat) = catalog() else { return };
    let pos = [-9000.0, 400.0, 90.0];
    assert!(
        cat.debug_param(12, NOON)
            .starts_with("LightParams 12 @ time 1440")
    );
    assert_eq!(cat.debug_param(12, NOON).lines().count(), 25);
    assert!(
        cat.debug_slots(EASTERN_KINGDOMS, pos, NOON)
            .contains("storm-underwater")
    );
    assert!(
        cat.debug_blend(EASTERN_KINGDOMS, pos, NOON)
            .contains("=> blended")
    );
    assert_eq!(cat.debug_param(0, NOON), "(LightParams ids are 1-based)\n");
}
