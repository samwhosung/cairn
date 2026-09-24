//! One creature display stood on the ground and aged: the subject of a display shot.

use std::time::Duration;

use avian3d::prelude::SpatialQuery;
use bevy::asset::RecursiveDependencyLoadState;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use world::collision::{CollisionResidency, WorldCollision};
use world::coords::wow_to_bevy;
use world::unit::{CharacterTables, UnitBody, UnitShade, UnitSystems};
use world::{Install, M2Model, Residency, WorldCamera, WorldSystems};

use crate::shot::ReadyToShoot;

pub const FRAME_STEP: Duration = Duration::from_nanos(16_666_667);
const SEAT_REACH: f32 = 500.0;

/// A display to stand at `at`, `scale` times its model's size, and shoot from the orbit `az`,
/// `el`, `dist` around the point a yard above its feet, `age` seconds after it appears.
#[derive(Resource, Clone, Copy, Debug, PartialEq)]
pub struct Fixture {
    pub display: u32,
    pub age: f32,
    pub scale: f32,
    /// WoW world coordinates; the display stands on whatever is below.
    pub at: Vec3,
    pub az_deg: f32,
    pub el_deg: f32,
    pub dist: f32,
}

#[derive(Default)]
enum Subject {
    #[default]
    Unresolved,
    NoModel,
    Body(Box<UnitBody>),
}

#[derive(Resource, Default)]
struct Stage {
    body: Subject,
    preload: Vec<UntypedHandle>,
    seat: Option<Vec3>,
    root: Option<Entity>,
    born: Option<f32>,
    clock_released: bool,
}

pub struct FixturePlugin(pub Fixture);

impl Plugin for FixturePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.0)
            .insert_resource(TimeUpdateStrategy::ManualDuration(FRAME_STEP))
            .init_resource::<Stage>()
            .add_systems(Startup, hold_clock)
            .add_systems(
                Update,
                (resolve_body, seat, spawn_subject, place_camera)
                    .chain()
                    .before(WorldSystems)
                    .before(UnitSystems),
            )
            .add_systems(Last, gate_clock);
    }
}

fn hold_clock(mut clock: ResMut<'_, Time<Virtual>>) {
    clock.pause();
}

fn resolve_body(
    tables: Option<Res<'_, CharacterTables>>,
    install: Res<'_, Install>,
    fixture: Res<'_, Fixture>,
    server: Res<'_, AssetServer>,
    mut images: ResMut<'_, Assets<Image>>,
    mut stage: ResMut<'_, Stage>,
) {
    if !matches!(stage.body, Subject::Unresolved) {
        return;
    }
    let Some(tables) = tables else {
        return;
    };
    let Some(body) = tables.display_body(fixture.display, &install.0, &mut images, &server) else {
        warn!(
            "display {} names no model: the shot stands nothing",
            fixture.display
        );
        stage.body = Subject::NoModel;
        return;
    };
    let mut preload = vec![server.load::<M2Model>(world::m2_url(&body.model)).untyped()];
    for worn in body.character.iter().flat_map(|c| &c.worn) {
        preload.push(server.load::<M2Model>(world::m2_url(&worn.model)).untyped());
        if let Some(t) = &worn.object_texture {
            let url = world::texture_url(t, world::Repeat::BOTH);
            preload.push(server.load::<Image>(url).untyped());
        }
    }
    stage.preload = preload;
    stage.body = Subject::Body(Box::new(body));
}

fn seat(
    fixture: Res<'_, Fixture>,
    residency: Res<'_, CollisionResidency>,
    spatial: SpatialQuery<'_, '_>,
    mut stage: ResMut<'_, Stage>,
) {
    if stage.seat.is_some() || !residency.settled() {
        return;
    }
    let mut pos = wow_to_bevy(fixture.at.to_array());
    if let Some(hit) = spatial.cast_ray(
        pos,
        Dir3::NEG_Y,
        SEAT_REACH,
        true,
        &WorldCollision::body_filter(),
    ) {
        pos.y -= hit.distance;
    }
    stage.seat = Some(pos);
}

fn spawn_subject(
    mut commands: Commands<'_, '_>,
    time: Res<'_, Time>,
    fixture: Res<'_, Fixture>,
    mut stage: ResMut<'_, Stage>,
) {
    if !stage.clock_released || stage.born.is_some() {
        return;
    }
    stage.born = Some(time.elapsed_secs());
    let (Some(seat), Subject::Body(body)) = (stage.seat, &stage.body) else {
        return;
    };
    let body = UnitBody::clone(body);
    let root = commands
        .spawn((
            Transform::from_translation(seat).with_scale(Vec3::splat(fixture.scale)),
            Visibility::default(),
            body,
            UnitShade::default(),
        ))
        .id();
    stage.root = Some(root);
}

pub fn orbit_transform(feet: Vec3, az_deg: f32, el_deg: f32, dist: f32) -> Transform {
    let look = feet + Vec3::Y;
    let orbit =
        Quat::from_rotation_y(az_deg.to_radians()) * Quat::from_rotation_x(-el_deg.to_radians());
    Transform::from_translation(look + orbit * (Vec3::Z * dist)).looking_at(look, Vec3::Y)
}

fn place_camera(
    fixture: Res<'_, Fixture>,
    stage: Res<'_, Stage>,
    roots: Query<'_, '_, &Transform, Without<WorldCamera>>,
    mut camera: Query<'_, '_, &mut Transform, With<WorldCamera>>,
) {
    let feet = stage
        .root
        .and_then(|r| roots.get(r).ok())
        .map(|t| t.translation)
        .or(stage.seat)
        .unwrap_or_else(|| wow_to_bevy(fixture.at.to_array()));
    let placed = orbit_transform(feet, fixture.az_deg, fixture.el_deg, fixture.dist);
    for mut t in &mut camera {
        if *t != placed {
            *t = placed;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn gate_clock(
    fixture: Res<'_, Fixture>,
    residency: Res<'_, Residency>,
    collision: Res<'_, CollisionResidency>,
    server: Res<'_, AssetServer>,
    images: Res<'_, Assets<Image>>,
    time: Res<'_, Time>,
    mut clock: ResMut<'_, Time<Virtual>>,
    mut stage: ResMut<'_, Stage>,
    mut ready: ResMut<'_, ReadyToShoot>,
) {
    if ready.0 {
        return;
    }
    if let Some(born) = stage.born {
        if time.elapsed_secs() - born >= fixture.age {
            clock.pause();
            ready.0 = true;
        }
        return;
    }
    if stage.clock_released {
        return;
    }
    let preloaded = match &stage.body {
        Subject::NoModel => true,
        Subject::Body(_) => stage.preload.iter().all(|h| {
            matches!(
                server.recursive_dependency_load_state(h.id()),
                RecursiveDependencyLoadState::Loaded | RecursiveDependencyLoadState::Failed(_)
            )
        }),
        Subject::Unresolved => false,
    };
    let dress = match &stage.body {
        Subject::Body(b) => b.character.as_ref(),
        _ => None,
    };
    let textures_ready = dress.is_none_or(|c| {
        [&c.body, &c.hair, &c.skin_extra, &c.object]
            .into_iter()
            .flatten()
            .all(|h| images.contains(h) || server.load_state(h).is_failed())
    });
    if residency.settled()
        && collision.settled()
        && stage.seat.is_some()
        && preloaded
        && textures_ready
    {
        clock.unpause();
        stage.clock_released = true;
    }
}
