//! Creatures and characters: a body's model dressed in its skins, skinned to its skeleton and
//! lit by its own shade outdoors and by the room it stands in indoors.

mod attach;
mod body;
mod drive;
mod fade;
mod light;
mod look;
mod motion;
mod shade;
mod twist;

use bevy::prelude::*;

use body::MeshCache;
pub use body::{BodyDressed, BodyModel, BodyPart, CharacterDress, UnitBody, WornModel};
pub use drive::UnitDriver;
pub use fade::{UnitAlpha, UnitAppear};
pub use look::{BodySkin, CharacterLook, CharacterTables};
pub use motion::{UnitMotion, move_flags};
pub use shade::UnitShade;
pub use twist::BodyTwist;

/// A unit's per-frame work in `Update`: dress what arrived, hang what it wears, drive its
/// animation from its movement, light it by where it stands, ramp its shade, carry its alpha.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnitSystems;

pub(crate) struct UnitPlugin;

impl Plugin for UnitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeshCache>()
            .add_systems(Startup, load_tables)
            .add_systems(
                Update,
                (
                    body::dress_bodies,
                    attach::attach_worn,
                    drive::drive_units,
                    light::classify_unit_light.after(crate::portal::compute_wmo_pvs),
                    fade::apply_unit_look,
                    shade::update_unit_shade,
                    fade::sync_depth_primes,
                )
                    .chain()
                    .in_set(UnitSystems),
            )
            .add_systems(
                PostUpdate,
                twist::apply_body_twist.in_set(crate::rig::PosePost),
            );
    }
}

fn load_tables(
    mut commands: Commands<'_, '_>,
    install: Res<'_, crate::Install>,
    loaded: Option<Res<'_, CharacterTables>>,
) {
    if loaded.is_some() {
        return;
    }
    match CharacterTables::load(&install) {
        Ok(tables) => {
            commands.insert_resource(tables);
        }
        Err(e) => warn!("no character tables, so no creature or character bodies: {e}"),
    }
}
