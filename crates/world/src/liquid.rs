//! The world's liquids: the animated surfaces of lakes, rivers, the sea, buildings' pools, magma
//! and slime, and where they are.

mod frames;
mod query;
mod spatial;
mod surface;

use bevy::prelude::*;

pub use query::{
    LiquidClaim, LiquidGrid, LiquidHit, LiquidSource, WmoPool, liquid_at, submersion_claim_at,
    surfaces_at, water_surface_at, wet_footprint,
};
pub use spatial::{SpatialIndex, WaterIndex};
pub(crate) use surface::{LiquidAssets, spawn_adt_liquids, spawn_wmo_liquids};
pub use surface::{LiquidExtension, LiquidMaterial};

/// Whether the liquids' frame flip and scroll run on the frame clock. A shot holds them at their
/// first frame, so every run of it draws the same one.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LiquidClock {
    #[default]
    Running,
    Frozen,
}

/// A water surface the wading foam can lie on; magma and slime carry none.
#[derive(Component)]
pub struct FoamPatch;

pub(crate) struct LiquidPlugin;

impl Plugin for LiquidPlugin {
    fn build(&self, app: &mut App) {
        surface::plugin(app);
        app.init_resource::<WaterIndex>()
            .add_systems(Startup, surface::setup_liquid)
            .add_systems(PreUpdate, spatial::maintain_water_index);
    }
}
