//! World of Warcraft 1.12.1 in Bevy: its files as assets, its world drawn as the client draws it.
#![allow(
    clippy::needless_pass_by_value,
    reason = "Bevy hands systems their parameters by value"
)]

mod adt;
mod atmosphere;
mod billboard;
mod celestial;
mod clouds;
pub mod collision;
pub mod coords;
pub mod doodad_sound;
mod glow;
mod ground;
mod horizon;
mod layers;
mod light;
mod m2;
mod map;
mod model;
mod model_material;
mod models;
mod placements;
mod portal;
mod probes;
pub mod rig;
pub mod rig_events;
mod room;
mod sh;
mod sky;
mod sky_order;
mod skybox;
mod source;
mod stream;
mod terrain;
mod texture;
pub mod unit;
mod view;
mod visibility;
mod wdt;
mod wmo;

use bevy::asset::AssetApp;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;

pub use adt::AdtTile;
pub use clouds::CloudClock;
pub use glow::FullScreenGlow;
pub use light::{Fog, SceneLight};
pub use m2::M2Model;
pub use map::CurrentMap;
pub use model::{BillboardInfo, ModelSubmesh};
pub use placements::{
    GLOBAL_WMO_ID, PlacedModel, Placement, Placements, PropPlacement, prop_placements,
};
pub use source::{Install, MPQ_SOURCE, Repeat, m2_url, register_source, texture_url, wmo_url};
pub use texture::{blp_image, rgba_image};
pub use view::{FARCLIP, FOV_Y, NEARCLIP, PROJECTION_FAR, WorldCamera, world_camera};
pub use wdt::WdtIndex;
pub use wmo::{DoodadBase, WmoGroupNav, WmoModel};

/// Registers the loaders for BLP textures, WDT and ADT map files, and M2 and WMO models. Add it
/// after `DefaultPlugins`: their render plugin says whether the GPU takes BC textures when it
/// finishes, and textures decode to RGBA8 without that answer.
pub struct LoadersPlugin;

impl Plugin for LoadersPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AdtTile>()
            .init_asset::<WdtIndex>()
            .init_asset::<M2Model>()
            .init_asset::<WmoModel>()
            .register_asset_loader(m2::M2Loader)
            .register_asset_loader(wmo::WmoLoader);
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

/// Draws the world around the [`WorldCamera`]: terrain to the far clip with the doodads and
/// buildings the map places on it, the WDL horizon past it and the sky behind, lit by the
/// [`SceneLight`]. Needs [`LoadersPlugin`] and the [`Install`], [`CurrentMap`] and [`TimeOfDay`]
/// resources.
pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            glow::GlowPlugin,
            light::LightBufferPlugin,
            terrain::TerrainMaterialPlugin,
            model_material::ModelMaterialPlugin,
            rig::RigPlugin,
            unit::UnitPlugin,
            probes::ProbePlugin,
            horizon::HorizonPlugin,
            sky::SkyPlugin,
            clouds::CloudsPlugin,
            celestial::CelestialPlugin,
            skybox::SkyboxPlugin,
        ))
        .init_resource::<Residency>()
        .init_resource::<stream::Streamer>()
        .init_resource::<Placements>()
        .init_resource::<models::Furnished>()
        .init_resource::<room::CameraRoom>()
        .init_resource::<room::RoomCrossfade>()
        .add_message::<rig_events::AnimEvent>()
        .add_systems(Startup, atmosphere::load_catalog)
        .add_systems(
            Update,
            (
                atmosphere::resolve_light.after(portal::compute_wmo_pvs),
                stream::stream_terrain,
                horizon::stream_horizon,
                (
                    placements::track_placements,
                    models::furnish,
                    portal::compute_wmo_pvs,
                    visibility::apply_model_visibility,
                    doodad_sound::fire_sound_host_events,
                )
                    .chain()
                    .after(stream::stream_terrain),
            )
                .in_set(WorldSystems),
        )
        .add_systems(
            Update,
            rig_events::fire_unit_events
                .after(unit::UnitSystems)
                .in_set(EventSystems),
        )
        .add_systems(
            PostUpdate,
            (
                billboard::face_billboards
                    .after(bevy::transform::TransformSystems::Propagate)
                    .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
                (
                    doodad_sound::reroll_sound_hosts,
                    doodad_sound::gate_sound_hosts,
                )
                    .chain(),
            ),
        );
    }
}

/// The world's per-frame work in `Update`; order whatever moves the camera before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldSystems;

/// The animation event keys fired this frame, after the units are driven; read them after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventSystems;

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
/// drawn or known to be missing, every model it places is drawn with its textures, the horizon
/// ring is up, and a painted sky a building shows is built or known to be missing.
#[allow(clippy::struct_excessive_bools)]
#[derive(Resource, Default, Debug)]
pub struct Residency {
    terrain: bool,
    models: bool,
    horizon: bool,
    skybox_pending: bool,
}

impl Residency {
    pub fn settled(&self) -> bool {
        self.terrain && self.models && self.horizon && !self.skybox_pending
    }
}
