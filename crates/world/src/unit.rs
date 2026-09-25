//! Creatures and characters: a body's model dressed in its skins, skinned to its skeleton, lit by
//! its own shade outdoors and by the room it stands in indoors, shadowed on the ground under it,
//! printing snow and sand where its feet plant, and breathing vapour in the cold.

mod attach;
mod batch_anim;
mod body;
mod breath;
mod drive;
mod fade;
mod footprints;
mod light;
mod look;
mod motion;
mod one_shot;
mod shade;
mod shadow;
mod twist;

use bevy::prelude::*;

use body::MeshCache;
pub(crate) use body::batch_look;
pub use body::{BodyDressed, BodyModel, BodyPart, CharacterDress, UnitBody, WornModel};
pub use drive::UnitDriver;
pub use fade::{UnitAlpha, UnitAppear};
pub use look::{BodySkin, CharacterLook, CharacterTables};
pub use motion::{Outcome, StandState, UnitAttack, UnitMotion, UnitShow, move_flags};
pub use shade::UnitShade;
pub use twist::BodyTwist;

/// A unit's per-frame work in `Update`: dress what arrived, hang what it wears, drive its
/// animation from its movement, light it by where it stands, draw each part at its alpha and
/// light, and ramp its shade.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnitSystems;

/// On the unit the viewer walks as.
#[derive(Component, Default)]
pub struct ViewerUnit;

pub(crate) struct UnitPlugin;

impl Plugin for UnitPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeshCache>()
            .init_resource::<footprints::Footprints>()
            .add_systems(
                Startup,
                (
                    load_tables,
                    shadow::load_texture,
                    footprints::load_tables,
                    breath::load_tables,
                ),
            )
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
                Update,
                (
                    shadow::update_shadows.after(UnitSystems),
                    footprints::spawn_footprints.after(crate::EventSystems),
                    (
                        breath::classify_breath,
                        breath::fire_breath,
                        one_shot::play_one_shots,
                    )
                        .chain()
                        .after(crate::EventSystems),
                ),
            )
            .add_systems(
                PostUpdate,
                (
                    twist::apply_body_twist.in_set(crate::rig::PosePost),
                    (shadow::push_shadows, footprints::push_footprints)
                        .after(crate::effects::begin_effect_frame),
                ),
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
