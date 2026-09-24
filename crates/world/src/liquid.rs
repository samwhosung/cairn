//! The world's liquids: the animated surfaces of lakes, rivers, the sea, buildings' pools, magma
//! and slime, where they are, and what the camera's eye is under.

mod drift;
mod foam;
mod frames;
mod interleave;
mod query;
#[cfg(test)]
mod real_data;
mod spatial;
mod surface;

use bevy::prelude::*;

pub(crate) use interleave::FarSide;
pub use query::{
    LiquidClaim, LiquidGrid, LiquidHit, LiquidSource, WmoPool, liquid_at, submersion_claim_at,
    surfaces_at, water_surface_at, world_grid,
};
pub use spatial::{SpatialIndex, WaterIndex};
pub(crate) use surface::{LiquidAssets, spawn_adt_liquids, spawn_wmo_liquids};
pub use surface::{LiquidExtension, LiquidMaterial};

/// A water surface the wading foam can lie on; magma and slime carry none.
#[derive(Component)]
pub struct FoamPatch;

pub(crate) use crate::submersion::{SubmergedEye, SubmersionVerdict, Underwater};

pub(crate) struct LiquidPlugin;

impl Plugin for LiquidPlugin {
    fn build(&self, app: &mut App) {
        surface::plugin(app);
        drift::plugin(app);
        foam::plugin(app);
        app.init_resource::<WaterIndex>()
            .init_resource::<Underwater>()
            .init_resource::<SubmergedEye>()
            .init_resource::<FarSide>()
            .add_systems(Startup, surface::setup_liquid)
            .add_systems(PreUpdate, spatial::maintain_water_index)
            .add_systems(
                Update,
                (
                    crate::submersion::detect_submersion
                        .in_set(SubmersionVerdict)
                        .after(crate::portal::compute_wmo_pvs),
                    interleave::classify_water_side
                        .after(SubmersionVerdict)
                        .after(crate::unit::UnitSystems)
                        .before(crate::visibility::apply_model_visibility),
                ),
            );
    }
}
