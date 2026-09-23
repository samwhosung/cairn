//! World of Warcraft 1.12.1 files in Bevy: the `mpq://` asset source, BLP textures and WoW's axes.

pub mod coords;
mod source;
mod texture;

use bevy::asset::AssetApp;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::{App, Plugin};

pub use source::{MPQ_SOURCE, Repeat, m2_url, register_source, texture_url, wmo_url};
pub use texture::blp_image;

use texture::BlpLoader;

/// Registers the BLP loader. Add it after `DefaultPlugins`: their render plugin says whether the
/// GPU takes BC textures when it finishes, and textures decode to RGBA8 without that answer.
pub struct LoadersPlugin;

impl Plugin for LoadersPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        let formats = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .map_or(CompressedImageFormats::NONE, |support| support.0);
        app.register_asset_loader(BlpLoader::new(formats));
    }
}
