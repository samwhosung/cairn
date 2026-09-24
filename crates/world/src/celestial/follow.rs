use bevy::camera::Projection;
use bevy::prelude::*;

use super::flare::{Allowance, FlareGate, Glare, MOON_RISE_PER_SEC, SUN_RISE_PER_SEC};
use super::materials::{CelestialMaterial, DiscSpan, StarMaterial};
use super::{Body, Celestial, SPRITE_SPHERE_YARDS, StarPatch};
use crate::clouds::{moon_halo, sun_clearance};
use crate::light::SceneLight;
use crate::view::WorldCamera;

const ONE_YARD_ON_SPRITE_SPHERE: f32 = 0.0833;
const DISC_SCALE_OF_FAR: f32 = 0.85;
const STARS_SCALE_OF_FAR: f32 = 0.88;
const WHITE_MOON_SIZE: f32 = 1.75;
const MOON_GLARE_SIZE: f32 = 2.0;
const GLARE_SWELLS_FROM_COS: f32 = 0.7;

type Camera<'w, 's> =
    Query<'w, 's, (&'static GlobalTransform, &'static Projection), With<WorldCamera>>;

type Sprites<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut GlobalTransform,
        &'static Celestial,
        &'static MeshMaterial3d<CelestialMaterial>,
        Option<&'static mut Glare>,
    ),
    Without<WorldCamera>,
>;

fn quantize(x: f32, steps: f32) -> f32 {
    (x * steps).round() / steps
}

fn far_plane(projection: &Projection) -> f32 {
    match projection {
        Projection::Perspective(p) => p.far,
        _ => crate::view::PROJECTION_FAR,
    }
}

fn turned_onto(forward: Vec3, to_body: Vec3) -> f32 {
    let cos = forward.dot(to_body);
    ((cos - GLARE_SWELLS_FROM_COS) / (1.0 - GLARE_SWELLS_FROM_COS)).clamp(0.0, 1.0)
}

fn disc_span(sin_elevation: f32, size: f32) -> DiscSpan {
    let elev = sin_elevation.clamp(-1.0, 1.0).asin();
    let half = (0.5 * size).atan();
    let q = |x: f32| quantize(x, 4096.0);
    DiscSpan {
        sin_bottom: q((elev - half).sin()),
        sin_top: q((elev + half).sin()),
    }
}

fn write_if_changed<M: Asset>(
    materials: &mut Assets<M>,
    handle: &Handle<M>,
    differs: impl Fn(&M) -> bool,
    write: impl FnOnce(&mut M),
) {
    if materials.get(handle).is_some_and(differs)
        && let Some(m) = materials.get_mut(handle)
    {
        write(m);
    }
}

struct Place {
    at: Vec3,
    toward: Vec3,
    scale: f32,
}

fn place(transform: &mut Transform, global: &mut GlobalTransform, p: &Place) {
    *transform = Transform {
        translation: p.at,
        rotation: Quat::from_rotation_arc(Vec3::Z, -p.toward),
        scale: Vec3::splat(p.scale),
    };
    *global = GlobalTransform::from(*transform);
}

fn place_disc(
    materials: &mut Assets<CelestialMaterial>,
    handle: &Handle<CelestialMaterial>,
    tint: Option<Color>,
    span: DiscSpan,
) {
    write_if_changed(
        materials,
        handle,
        |m| tint.is_some_and(|t| m.base.base_color != t) || m.extension.span != span,
        |m| {
            if let Some(t) = tint {
                m.base.base_color = t;
            }
            m.extension.span = span;
        },
    );
}

fn place_glare(
    materials: &mut Assets<CelestialMaterial>,
    handle: &Handle<CelestialMaterial>,
    color: Color,
) {
    write_if_changed(
        materials,
        handle,
        |m| m.base.base_color != color,
        |m| m.base.base_color = color,
    );
}

pub(super) fn follow_sun(
    camera: Camera<'_, '_>,
    light: Res<'_, SceneLight>,
    gate: FlareGate<'_>,
    mut materials: ResMut<'_, Assets<CelestialMaterial>>,
    mut sprites: Sprites<'_, '_>,
) {
    let Ok((eye, projection)) = camera.single() else {
        return;
    };
    let to_sun = light.visible_sun.normalize_or_zero();
    if to_sun == Vec3::ZERO {
        return;
    }
    let (cam, dist) = (eye.translation(), far_plane(projection) * DISC_SCALE_OF_FAR);
    let f = turned_onto(*eye.forward(), to_sun);
    let [r, g, b] = light.celestial_tint.map(|c| quantize(c, 255.0));
    let size = ONE_YARD_ON_SPRITE_SPHERE * light.sun_disc_scale;
    let allow = Allowance {
        hour: light.sun_flare,
        cloud_clearance: sun_clearance(gate.clouds.coverage_toward(to_sun)),
        disc_half_angle: (0.5 * size).atan(),
        rise_per_sec: SUN_RISE_PER_SEC,
    };
    for (mut transform, mut global, body, material, glare) in &mut sprites {
        match (body.0, glare) {
            (Body::SunDisc, _) => {
                let at = cam + to_sun * dist;
                let p = Place {
                    at,
                    toward: to_sun,
                    scale: size * dist,
                };
                place(&mut transform, &mut global, &p);
                let span = disc_span(to_sun.y, size);
                place_disc(
                    &mut materials,
                    &material.0,
                    Some(Color::srgb(r, g, b)),
                    span,
                );
            }
            (Body::SunGlare, Some(mut glare)) => {
                let at = cam + to_sun * SPRITE_SPHERE_YARDS;
                let p = Place {
                    at,
                    toward: to_sun,
                    scale: 3.0 + 17.0 * f,
                };
                place(&mut transform, &mut global, &p);
                let shine = gate.ease(&mut glare, cam, to_sun, &allow);
                let a = quantize((0.5 + 0.5 * f) * shine, 255.0);
                place_glare(&mut materials, &material.0, Color::srgba(r, g, b, a));
            }
            _ => {}
        }
    }
}

pub(super) fn follow_moons(
    camera: Camera<'_, '_>,
    light: Res<'_, SceneLight>,
    gate: FlareGate<'_>,
    mut materials: ResMut<'_, Assets<CelestialMaterial>>,
    mut sprites: Sprites<'_, '_>,
) {
    let Ok((eye, projection)) = camera.single() else {
        return;
    };
    let (cam, dist) = (eye.translation(), far_plane(projection) * DISC_SCALE_OF_FAR);
    let [r, g, b] = light.celestial_tint.map(|c| quantize(c, 255.0));
    let to_moon = light.moon.normalize_or_zero();
    let size = ONE_YARD_ON_SPRITE_SPHERE * WHITE_MOON_SIZE * light.moon_disc_scale;
    let allow = Allowance {
        hour: light.moon_flare,
        cloud_clearance: moon_halo(gate.clouds.coverage_toward(to_moon)),
        disc_half_angle: (0.5 * size).atan(),
        rise_per_sec: MOON_RISE_PER_SEC,
    };
    for (mut transform, mut global, body, material, glare) in &mut sprites {
        let toward = match body.0 {
            Body::SecondMoon => light.moon02.normalize_or_zero(),
            Body::MoonDisc | Body::MoonGlare => to_moon,
            Body::SunDisc | Body::SunGlare => continue,
        };
        if toward == Vec3::ZERO {
            continue;
        }
        match (body.0, glare) {
            (Body::MoonDisc, _) => {
                let p = Place {
                    at: cam + toward * dist,
                    toward,
                    scale: size * dist,
                };
                place(&mut transform, &mut global, &p);
                let span = disc_span(toward.y, size);
                place_disc(
                    &mut materials,
                    &material.0,
                    Some(Color::srgb(r, g, b)),
                    span,
                );
            }
            (Body::SecondMoon, _) => {
                let size = ONE_YARD_ON_SPRITE_SPHERE * light.moon02_disc_scale;
                let p = Place {
                    at: cam + toward * dist,
                    toward,
                    scale: size * dist,
                };
                place(&mut transform, &mut global, &p);
                place_disc(&mut materials, &material.0, None, disc_span(toward.y, size));
            }
            (Body::MoonGlare, Some(mut glare)) => {
                let scale = MOON_GLARE_SIZE * light.moon_disc_scale;
                let p = Place {
                    at: cam + toward * SPRITE_SPHERE_YARDS,
                    toward,
                    scale,
                };
                place(&mut transform, &mut global, &p);
                let f = turned_onto(*eye.forward(), toward);
                let shine = gate.ease(&mut glare, cam, toward, &allow);
                let a = quantize((0.1 + 0.9 * f) * shine, 255.0);
                place_glare(&mut materials, &material.0, Color::srgba(r, g, b, a));
            }
            _ => {}
        }
    }
}

type Patches<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut GlobalTransform,
        &'static MeshMaterial3d<StarMaterial>,
        &'static StarPatch,
    ),
    Without<WorldCamera>,
>;

pub(super) fn follow_stars(
    camera: Camera<'_, '_>,
    light: Res<'_, SceneLight>,
    mut materials: ResMut<'_, Assets<StarMaterial>>,
    mut patches: Patches<'_, '_>,
) {
    let Ok((eye, projection)) = camera.single() else {
        return;
    };
    let byte = (light.star_alpha * 254.0 + 1.0).trunc();
    let night = if byte < 2.0 { 0.0 } else { byte / 255.0 };
    let scale = far_plane(projection) * STARS_SCALE_OF_FAR;
    for (mut transform, mut global, material, patch) in &mut patches {
        *transform = Transform::from_translation(eye.translation()).with_scale(Vec3::splat(scale));
        *global = GlobalTransform::from(*transform);
        let color = Color::srgba(1.0, 1.0, 1.0, quantize(night * patch.authored_alpha, 255.0));
        write_if_changed(
            &mut materials,
            &material.0,
            |m| m.base.base_color != color,
            |m| m.base.base_color = color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::materials::DISC_HORIZON_FADE;
    use super::*;

    #[test]
    fn a_setting_disc_straddles_the_horizon_and_a_high_one_clears_the_fade() {
        let band_top = 1.0 / DISC_HORIZON_FADE;
        let up = disc_span(30_f32.to_radians().sin(), ONE_YARD_ON_SPRITE_SPHERE);
        assert!(up.sin_bottom > band_top && up.sin_top > up.sin_bottom);
        let setting = disc_span(2_f32.to_radians().sin(), 2.0 * ONE_YARD_ON_SPRITE_SPHERE);
        assert!(setting.sin_bottom < 0.0 && setting.sin_top > band_top);
        let level = disc_span(0.0, 0.1);
        assert!((level.sin_bottom + level.sin_top).abs() < 1e-6);
    }

    #[test]
    fn the_glare_swells_only_within_45_degrees_of_the_view() {
        assert!(turned_onto(Vec3::X, Vec3::Y).abs() < 1e-6);
        assert!((turned_onto(Vec3::X, Vec3::X) - 1.0).abs() < 1e-6);
        assert_eq!((1.0 - GLARE_SWELLS_FROM_COS).to_bits(), 0.3_f32.to_bits());
    }
}
