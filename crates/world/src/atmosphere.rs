use bevy::prelude::*;
use light::{Atmosphere, LightCatalog, Submersion, daynight};

use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::light::SceneLight;
use crate::view::{FARCLIP, WorldCamera};
use crate::{CurrentMap, Install, TimeOfDay};

#[derive(Resource)]
pub(crate) struct Catalog(Option<LightCatalog>);

pub(crate) fn load_catalog(mut commands: Commands<'_, '_>, install: Res<'_, Install>) {
    let catalog = LightCatalog::load(&install.0)
        .inspect_err(|e| warn!("no lighting tables, lighting a neutral day: {e}"))
        .ok();
    commands.insert_resource(Catalog(catalog));
}

/// The clear colour takes the fog's gamma values raw, like every colour in the frame.
pub(crate) fn resolve_light(
    catalog: Option<Res<'_, Catalog>>,
    map: Res<'_, CurrentMap>,
    time: Res<'_, TimeOfDay>,
    camera: Query<'_, '_, &Transform, With<WorldCamera>>,
    mut light: ResMut<'_, SceneLight>,
    mut clear: ResMut<'_, ClearColor>,
) {
    let (Some(catalog), Ok(camera)) = (catalog, camera.single()) else {
        return;
    };
    let eye = bevy_to_wow(camera.translation);
    let (stormy, ghost) = (false, false);
    let atmosphere = catalog.0.as_ref().map_or(Atmosphere::DEFAULT, |c| {
        c.sample(
            map.id,
            eye,
            time.half_minutes(),
            stormy,
            Submersion::Dry,
            ghost,
        )
    });
    let resolved = scene_light(&atmosphere, time.minute);
    if *light != resolved {
        *light = resolved;
    }
    let [r, g, b] = resolved.fog_color;
    let fog = Color::linear_rgb(r, g, b);
    if clear.0 != fog {
        clear.0 = fog;
    }
}

fn scene_light(atmosphere: &Atmosphere, minute: u32) -> SceneLight {
    let minute = minute as f32;
    let fog_end = atmosphere.fog_end.min(FARCLIP);
    SceneLight {
        ambient: atmosphere.ambient,
        diffuse: atmosphere.sun_diffuse,
        specular: atmosphere.sun_color,
        sun: wow_to_bevy(daynight::sun_direction(minute)).normalize(),
        fog_color: atmosphere.fog_color,
        fog_start: atmosphere.fog_start_frac * fog_end,
        fog_end,
        sky: atmosphere.sky,
        sky_warp: daynight::sky_warp(minute, atmosphere.highlight_sky),
        visible_sun: wow_to_bevy(daynight::celestial_sun_direction(minute)).normalize(),
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::light::rows;

    /// Goldshire lies in no light sphere, so noon is Azeroth's own `LightParams` 12 at one of its
    /// keys; its 500-yard fog end clips to the far clip.
    #[test]
    fn goldshire_at_noon_packs_the_rows_by_hand() {
        let Some(data) = std::env::var_os("WOW_DATA") else {
            eprintln!("skipped: WOW_DATA is not set");
            return;
        };
        let chain = mpq::Chain::open(data).expect("open the chain");
        let catalog = LightCatalog::load(&chain).expect("the tables load");
        let eye = [-9439.1, 71.2, 68.0];
        let atmosphere = catalog.sample(0, eye, 1440, false, Submersion::Dry, false);
        let rows = rows(&scene_light(&atmosphere, 720));

        let rgbw = |[r, g, b]: [u8; 3], w: f32| {
            let v = |byte: u8| f32::from(byte) / 255.0;
            [v(r), v(g), v(b), w]
        };
        assert_eq!(rows[0], rgbw([104, 130, 154], 0.0));
        assert_eq!(rows[1], rgbw([255, 136, 0], 0.0));
        assert_eq!(rows[3], rgbw([255, 247, 222], 20.0));
        assert_eq!(rows[4], rgbw([77, 120, 143], 1.0));
        assert_eq!(rows[5], [0.25 * 350.0, 350.0, 0.0, 350.0]);

        let (phi, theta) = (2.216_568_2_f32, 225.0_f32.to_radians());
        let toward = [phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos()];
        let sun = wow_to_bevy(toward);
        for (got, want) in rows[2][..3].iter().zip(sun.to_array()) {
            assert!((got - want).abs() < 1e-6, "{:?} vs {sun}", rows[2]);
        }
    }
}
