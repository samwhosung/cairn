//! World of Warcraft 1.12.1 in Bevy: its files as assets, its world drawn as the client draws it.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod adt;
mod atmosphere;
pub mod coords;
mod decode;
mod horizon;
mod layers;
mod light;
mod map;
mod sky;
mod source;
mod stream;
mod terrain;
mod texture;
mod view;
mod wdt;

use bevy::asset::AssetApp;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;

pub use adt::AdtTile;
pub use light::SceneLight;
pub use map::CurrentMap;
pub use source::{Install, MPQ_SOURCE, Repeat, m2_url, register_source, texture_url, wmo_url};
pub use texture::blp_image;
pub use view::{FARCLIP, FOV_Y, NEARCLIP, PROJECTION_FAR, WorldCamera, world_camera};
pub use wdt::WdtIndex;

/// Registers the loaders for BLP textures and for WDT and ADT map files. Add it after
/// `DefaultPlugins`: their render plugin says whether the GPU takes BC textures when it finishes,
/// and textures decode to RGBA8 without that answer.
pub struct LoadersPlugin;

impl Plugin for LoadersPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AdtTile>().init_asset::<WdtIndex>();
    }

    fn finish(&self, app: &mut App) {
        let formats = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .map_or(CompressedImageFormats::NONE, |support| support.0);
        app.register_asset_loader(texture::BlpLoader::new(formats))
            .register_asset_loader(adt::AdtLoader::new(formats))
            .register_asset_loader(wdt::WdtLoader);
    }
}

/// Draws the world around the [`WorldCamera`]: terrain to the far clip, the WDL horizon past it and
/// the sky behind, lit by the [`SceneLight`]. Needs [`LoadersPlugin`] and the [`Install`],
/// [`CurrentMap`] and [`TimeOfDay`] resources.
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            decode::DecodePlugin,
            light::LightBufferPlugin,
            terrain::TerrainMaterialPlugin,
            horizon::HorizonPlugin,
            sky::SkyPlugin,
        ))
        .init_resource::<Residency>()
        .init_resource::<stream::Streamer>()
        .add_systems(Startup, atmosphere::load_catalog)
        .add_systems(
            Update,
            (
                atmosphere::resolve_light,
                stream::stream_terrain,
                horizon::stream_horizon,
            )
                .in_set(WorldSystems),
        );
    }
}

/// The world's per-frame work in `Update`; order whatever moves the camera before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldSystems;

/// The game minute of the day, `0..1440`, the world is lit for.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeOfDay {
    pub minute: u32,
}

impl TimeOfDay {
    /// The time in the lighting tables' unit.
    pub fn half_minutes(self) -> u32 {
        self.minute * 2
    }
}

/// Whether everything around the camera has arrived: every terrain tile the far clip reaches is
/// drawn or known to be missing, and the horizon ring is up.
#[derive(Resource, Default, Debug)]
pub struct Residency {
    terrain: bool,
    horizon: bool,
}

impl Residency {
    pub fn settled(&self) -> bool {
        self.terrain && self.horizon
    }
}
