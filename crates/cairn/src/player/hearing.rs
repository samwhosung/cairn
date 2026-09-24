//! What the world's sound needs of the player: where the body stands, what it is, and where its
//! head is and faces.

use bevy::prelude::*;
use sound::{ListenerCharacter, Listening, SoundBody};
use world::interior::Viewer;
use world::unit::{CharacterTables, UnitBody};

use super::Mode;
use super::body::{PlayerBody, PlayerLook};
use super::camera::{CameraPivot, model_pivot_height};
use super::state::Player;
use super::swim::swim_enter_depth;

/// The head's height over the feet before the body's model has loaded.
const HEAD_FALLBACK: f32 = 1.8;

type Body<'a> = (
    Entity,
    &'a Transform,
    Option<&'a CameraPivot>,
    Option<&'a SoundBody>,
    Has<UnitBody>,
);

/// Walking, the body and its head; flying, neither, so the world is placed and heard from the
/// camera.
#[allow(clippy::too_many_arguments)]
pub fn publish_body(
    mut commands: Commands<'_, '_>,
    mode: Res<'_, Mode>,
    player: Res<'_, Player>,
    look: Res<'_, PlayerLook>,
    tables: Option<Res<'_, CharacterTables>>,
    bodies: Query<'_, '_, Body<'_>, With<PlayerBody>>,
    viewer: Option<ResMut<'_, Viewer>>,
    listener: Option<ResMut<'_, ListenerCharacter>>,
) {
    let Ok((entity, transform, pivot, sounding, dressed)) = bodies.single() else {
        return;
    };
    let walking = *mode == Mode::Walk;
    if let Some(mut viewer) = viewer {
        viewer.set_if_neq(Viewer {
            body: walking.then_some(player.pos),
            settled: walking && !player.settling,
        });
    }
    if let Some(mut listener) = listener {
        let head = pivot.map_or(HEAD_FALLBACK, |p| {
            model_pivot_height(*p, transform.scale.x, false)
        });
        listener.set_if_neq(ListenerCharacter(
            walking.then_some((player.pos + Vec3::Y * head, player.face_yaw)),
        ));
    }
    if !walking {
        return;
    }
    let display = tables
        .as_deref()
        .and_then(|t| t.create.body_display(look.0.race, look.0.sex));
    let wanted = display.map(|display| SoundBody {
        display,
        collision_height: player.collision_height,
        wade_max: swim_enter_depth(player.collision_height),
    });
    if dressed
        && let Some(wanted) = wanted
        && sounding != Some(&wanted)
    {
        commands.entity(entity).insert((wanted, Listening));
    }
}
