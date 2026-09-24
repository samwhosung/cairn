mod flare;
mod follow;
mod materials;
mod setup;

use bevy::asset::embedded_asset;
use bevy::camera::visibility::VisibilitySystems;
use bevy::pbr::MaterialPlugin;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

pub use materials::{CelestialMaterial, StarMaterial};

pub(crate) const SPRITE_SPHERE_YARDS: f32 = 12.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Body {
    SunDisc,
    SunGlare,
    MoonDisc,
    MoonGlare,
    SecondMoon,
}

#[derive(Component)]
struct Celestial(Body);

#[derive(Component)]
struct StarPatch {
    authored_alpha: f32,
}

pub(crate) struct CelestialPlugin;

impl Plugin for CelestialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "celestial.wgsl");
        embedded_asset!(app, "star.wgsl");
        app.add_plugins((
            MaterialPlugin::<CelestialMaterial>::default(),
            MaterialPlugin::<StarMaterial>::default(),
        ))
        .add_systems(Startup, setup::spawn_bodies)
        .add_systems(
            PostUpdate,
            (
                follow::follow_sun,
                follow::follow_moons,
                follow::follow_stars,
            )
                .after(TransformSystems::Propagate)
                .before(VisibilitySystems::CheckVisibility),
        );
    }
}
