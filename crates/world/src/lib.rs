//! World of Warcraft 1.12.1 files in Bevy: the `mpq://` source, textures, map tiles and WoW's axes.

mod adt;
pub mod coords;
mod layers;
mod source;
mod texture;
mod wdt;

use bevy::asset::AssetApp;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::{App, Plugin};

pub use adt::AdtTile;
pub use source::{MPQ_SOURCE, Repeat, m2_url, register_source, texture_url, wmo_url};
pub use texture::blp_image;
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
