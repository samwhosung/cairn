use bevy::prelude::*;
use light::{Atmosphere, LightCatalog, Submersion, daynight};

use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::light::{Fog, SceneLight};
use crate::room::{CameraRoom, RoomCrossfade};
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_light(
    catalog: Option<Res<'_, Catalog>>,
    map: Res<'_, CurrentMap>,
    time: Res<'_, TimeOfDay>,
    clock: Res<'_, Time>,
    room: Res<'_, CameraRoom>,
    mut crossfade: ResMut<'_, RoomCrossfade>,
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
    let mut resolved = scene_light(&atmosphere, time.minute);
    resolved.room_fog = crossfade.blend(room.fog, resolved.room_fog, FARCLIP, clock.delta_secs());
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
    let day = f64::from(minute) / 1440.0;
    let minute = minute as f32;
    let fog_end = atmosphere.fog_end.min(FARCLIP);
    let toward = |d: [f32; 3]| wow_to_bevy(d).normalize();
    let visible_sun = toward(daynight::celestial_sun_direction(minute));
    let moon = toward(daynight::moon_direction(minute));
    let (moon02, moon02_disc_scale) = daynight::moon02_state(day);
    SceneLight {
        ambient: atmosphere.ambient,
        diffuse: atmosphere.sun_diffuse,
        specular: atmosphere.sun_color,
        sun: toward(daynight::sun_direction(minute)),
        fog_color: atmosphere.fog_color,
        fog_start: atmosphere.fog_start_frac * fog_end,
        fog_end,
        room_fog: Fog {
            color: atmosphere.fog_color,
            start: atmosphere.fog_start_frac * fog_end,
            end: fog_end,
        },
        sky: atmosphere.sky,
        sky_warp: daynight::sky_warp(minute, atmosphere.highlight_sky),
        visible_sun,
        night_glow: daynight::sidn_night_fraction(minute),
        glow: (atmosphere.glow * 255.0).floor() / 255.0,
        celestial_tint: atmosphere.sun_color,
        sun_disc_scale: daynight::sun_disc_scale(minute),
        sun_flare: daynight::sun_flare_dn(minute),
        moon,
        moon_disc_scale: daynight::moon_disc_scale(minute),
        moon_flare: daynight::moon_flare_dn(minute),
        moon02: toward(moon02),
        moon02_disc_scale,
        star_alpha: daynight::star_alpha(minute),
        cloud_density: atmosphere.cloud_density,
        cloud_colors: atmosphere.cloud_colors,
        cloud_glow_dir: if daynight::cloud_glow_is_sun(minute) {
            visible_sun
        } else {
            moon
        },
        cloud_glow: daynight::cloud_glow_track(minute),
        water_river: water(atmosphere.water_river, atmosphere.water_river_alpha),
        water_ocean: water(atmosphere.water_ocean, atmosphere.water_ocean_alpha),
    }
}

fn water(colors: [[f32; 3]; 2], alphas: [f32; 2]) -> [[f32; 4]; 2] {
    std::array::from_fn(|i| {
        let [r, g, b] = colors[i];
        [r, g, b, alphas[i]]
    })
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
        assert_eq!(rows[0], rgbw([104, 130, 154], 1.0));
        assert_eq!(rows[1], rgbw([255, 136, 0], 1.0));
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
