//! The body the player walks as: a character dressed from its look, posed by the frame's
//! movement, and faded as the camera closes on it.

use bevy::prelude::*;
use world::unit::{
    BodyModel, CharacterLook, CharacterTables, UnitAlpha, UnitBody, UnitMotion, UnitShade,
    ViewerUnit,
};
use world::{Install, M2Model};

use super::camera::CameraPivot;
use super::state::{DEFAULT_COLLISION_HEIGHT, Player};

#[derive(Component)]
pub struct PlayerBody;

#[derive(Resource, Clone, Debug)]
pub struct PlayerLook(pub CharacterLook);

pub fn spawn_body(mut commands: Commands<'_, '_>) {
    commands.spawn((
        PlayerBody,
        Transform::default(),
        Visibility::default(),
        UnitShade::default(),
        UnitMotion::default(),
        UnitAlpha::default(),
        ViewerUnit,
    ));
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn dress_body(
    mut commands: Commands<'_, '_>,
    tables: Option<Res<'_, CharacterTables>>,
    install: Res<'_, Install>,
    look: Res<'_, PlayerLook>,
    server: Res<'_, AssetServer>,
    mut images: ResMut<'_, Assets<Image>>,
    mut player: ResMut<'_, Player>,
    mut bodies: Query<
        '_,
        '_,
        (Entity, &mut Transform),
        (With<PlayerBody>, Without<UnitBody>, Without<CameraPivot>),
    >,
) {
    let Some(tables) = tables else {
        return;
    };
    for (entity, mut transform) in &mut bodies {
        if let Some((body, scale)) =
            character_body(&tables, &look.0, &install, &mut images, &server)
        {
            transform.scale = Vec3::splat(scale);
            commands.entity(entity).insert(body);
            let display = tables.create.body_display(look.0.race, look.0.sex);
            player.collision_height = collision_height(&tables, display, scale);
        } else {
            warn!("no body for {:?}: the player walks unseen", look.0);
            commands.entity(entity).insert(CameraPivot::FLOOR);
        }
    }
}

#[allow(clippy::type_complexity)]
pub fn pivot_on_model(
    mut commands: Commands<'_, '_>,
    m2s: Res<'_, Assets<M2Model>>,
    bodies: Query<'_, '_, (Entity, &BodyModel), (With<PlayerBody>, Without<CameraPivot>)>,
) {
    for (entity, model) in &bodies {
        if let Some(m2) = m2s.get(&model.0) {
            commands.entity(entity).insert(CameraPivot::of(Some(m2)));
        }
    }
}

/// The body a character of `look` is drawn with, and the scale its race's display takes.
pub fn character_body(
    tables: &CharacterTables,
    look: &CharacterLook,
    install: &Install,
    images: &mut Assets<Image>,
    server: &AssetServer,
) -> Option<(UnitBody, f32)> {
    let body = tables.player_body(look, &install.0, images, server)?;
    let display = tables.create.body_display(look.race, look.sex);
    Some((body, new_character_scale(tables, display)))
}

fn new_character_scale(tables: &CharacterTables, display: Option<u32>) -> f32 {
    display
        .and_then(|d| tables.creatures.model_scale(d))
        .filter(|s| *s > 0.0)
        .unwrap_or(1.0)
}

fn collision_height(tables: &CharacterTables, display: Option<u32>, scale: f32) -> f32 {
    let raw = display
        .and_then(|d| tables.creatures.collision_height(d))
        .filter(|h| *h > 0.0)
        .unwrap_or(DEFAULT_COLLISION_HEIGHT);
    let floor = display.and_then(|d| tables.creatures.display_scale(d));
    raw * floor.map_or(scale, |s| scale.max(s)).max(f32::MIN_POSITIVE)
}
