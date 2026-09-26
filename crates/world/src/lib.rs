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
mod clutter;
pub mod collision;
pub mod coords;
mod dbc_table;
mod decal;
mod doodad_anim;
mod doodad_events;
mod draw_order;
pub mod effects;
mod glow;
mod ground;
pub mod hands;
mod horizon;
pub mod interior;
mod layers;
mod light;
pub mod liquid;
mod m2;
mod map;
mod mat_anim_table;
mod model;
mod model_material;
mod models;
pub mod particles;
mod placements;
mod portal;
mod probes;
pub mod ribbons;
pub mod rig;
pub mod rig_events;
mod room;
mod sh;
pub mod sight;
mod sky;
mod sky_order;
mod skybox;
mod source;
mod stream;
pub mod submersion;
pub mod surface;
mod terrain;
mod texture;
pub mod unit;
mod view;
mod visibility;
mod wdt;
mod wmo;
mod wmo_areas;

use bevy::asset::AssetApp;
use bevy::camera::visibility::VisibilitySystems;
use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
use bevy::prelude::*;

pub use adt::AdtTile;
pub use clouds::CloudClock;
pub use doodad_anim::DoodadAnimHost;
pub use glow::FullScreenGlow;
pub use light::{Fog, SceneLight};
pub use m2::M2Model;
pub use map::{Borrowed, CurrentMap};
pub use model::{BillboardInfo, ModelSubmesh};
pub use placements::{
    Filed, GLOBAL_WMO_ID, PlacedModel, Placement, PlacementEdits, Placements, PropPlacement,
    prop_placements,
};
pub use portal::WholeBuildings;
pub use source::{Install, MPQ_SOURCE, Repeat, m2_url, register_source, texture_url, wmo_url};
pub use texture::{blp_image, rgba_image};
pub use view::{FARCLIP, FOV_Y, NEARCLIP, PROJECTION_FAR, WorldCamera, world_camera};
pub use wdt::WdtIndex;
pub use wmo::{DoodadBase, WmoGroupNav, WmoModel, WmoRooms};
pub use wmo_areas::{WmoArea, WmoAreas};

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
            doodad_anim::DoodadAnimPlugin,
            unit::UnitPlugin,
            probes::ProbePlugin,
            horizon::HorizonPlugin,
            sky::SkyPlugin,
            clouds::CloudsPlugin,
            celestial::CelestialPlugin,
            skybox::SkyboxPlugin,
            liquid::LiquidPlugin,
        ))
        .add_plugins((
            draw_order::MeshSlabsPlugin,
            effects::EffectsPlugin,
            particles::ParticlePlugin,
            ribbons::RibbonPlugin,
        ))
        .init_resource::<Residency>()
        .init_resource::<LeftOut>()
        .init_resource::<stream::Streamer>()
        .init_resource::<clutter::Clutter>()
        .init_resource::<Placements>()
        .init_resource::<PlacementEdits>()
        .init_resource::<models::Furnished>()
        .init_resource::<room::CameraRoom>()
        .init_resource::<room::RoomCrossfade>()
        .init_resource::<interior::Viewer>()
        .init_resource::<interior::CurrentWmoInterior>()
        .init_resource::<interior::CurrentAreaInterior>()
        .init_resource::<interior::CurrentArea>()
        .init_resource::<interior::WmoGeneration>()
        .init_resource::<portal::WholeBuildings>()
        .init_resource::<portal::CameraInteriorClaim>()
        .init_resource::<portal::ExteriorWindows>()
        .add_message::<rig_events::AnimEvent>()
        .add_message::<unit::UnitAttack>()
        .add_systems(
            Startup,
            (
                atmosphere::load_catalog,
                wmo_areas::load,
                clutter::load_effects,
            ),
        )
        .add_systems(
            Update,
            (
                atmosphere::resolve_light
                    .after(portal::compute_wmo_pvs)
                    .after(submersion::SubmersionVerdict),
                stream::stream_terrain,
                horizon::stream_horizon,
                (
                    placements::track_placements,
                    models::furnish,
                    clutter::stream_clutter,
                    portal::compute_wmo_pvs,
                    visibility::apply_model_visibility,
                    interior::bump_wmo_generation,
                    interior::track_current_interior,
                    interior::track_area_interior,
                    interior::update_current_area,
                    interior::track_unit_rooms,
                    doodad_events::fire_doodad_events,
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
        .add_systems(Update, visibility::leave_out)
        .add_systems(
            PostUpdate,
            (
                billboard::face_billboards
                    .after(rig::RigFinalize)
                    .before(VisibilitySystems::CheckVisibility),
                draw_order::visible_in_id_order.after(VisibilitySystems::CheckVisibility),
            ),
        );
        mat_anim_table::plugin(app);
    }
}

/// The world's per-frame work in `Update`; order whatever moves the camera before it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorldSystems;

/// The units' animation event keys fired this frame, after the units are driven; read them after
/// it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventSystems;

/// Placements the world camera leaves undrawn, by the unique id the map's files place them under;
/// a building's own doodads go with it. They go on as if seen, and what they emit, light and water,
/// stays.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct LeftOut(pub std::collections::BTreeSet<u32>);

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
/// drawn or known to be missing, every model it places is drawn with its textures, the ground
/// clutter near the camera is built, the horizon ring is up, and a painted sky a building shows
/// is built or known to be missing.
#[allow(clippy::struct_excessive_bools)]
#[derive(Resource, Default, Debug)]
pub struct Residency {
    terrain: bool,
    models: bool,
    clutter: bool,
    horizon: bool,
    skybox_pending: bool,
}

impl Residency {
    pub fn settled(&self) -> bool {
        self.terrain && self.models && self.clutter && self.horizon && !self.skybox_pending
    }
}
