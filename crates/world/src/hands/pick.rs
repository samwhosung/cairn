use bevy::camera::RenderTarget;
use bevy::picking::PickingSystems;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{PointerId, PointerLocation};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use crate::sight::{Sight, Sighting};
use crate::view::{FARCLIP, NEARCLIP, WorldCamera};

/// A picking backend over [`Sight`]: a pointer on the world hovers one [`OnTheWorld`] entity, at
/// the nearest model batch drawn or terrain, and [`Pointed`] says which. The batches are those
/// drawn [`PICKS_FRAMES_LATE`] frames before. A backend of a higher order, a panel's, takes the
/// pointer first.
pub struct SightPickingPlugin;

pub const PICKS_FRAMES_LATE: u32 = 1;

impl Plugin for SightPickingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SightPicking>()
            .init_resource::<Pointed>()
            .add_systems(Startup, |mut commands: Commands<'_, '_>| {
                commands.spawn(OnTheWorld);
            })
            .add_systems(PreUpdate, sight_hits.in_set(PickingSystems::Backend));
    }
}

/// What a pointer on the world hovers, whatever it meets there, so that picking marks no entity
/// the world draws.
#[derive(Component)]
pub struct OnTheWorld;

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SightPicking {
    #[default]
    Models,
    ThroughToTheGround,
}

/// Where a pointer meets the world: what it points at, a model or the ground, and the ground
/// behind whatever that is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PointedAt {
    pub first: Option<Sighting>,
    pub ground: Option<Sighting>,
}

/// What each pointer met, as the backend last cast it.
#[derive(Resource, Default)]
pub struct Pointed(Vec<(PointerId, PointedAt)>);

impl Pointed {
    pub fn of(&self, pointer: PointerId) -> Option<&PointedAt> {
        self.0.iter().find(|(p, _)| *p == pointer).map(|(_, at)| at)
    }

    /// What `pointer` points at in the world, if picking left it on the world: nothing a panel
    /// took.
    pub fn over_world(
        &self,
        pointer: PointerId,
        hover: &HoverMap,
        world: Entity,
    ) -> Option<&PointedAt> {
        let on_world = hover.get(&pointer)?.contains_key(&world);
        on_world.then(|| self.of(pointer)).flatten()
    }
}

type Cameras<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static Camera,
        &'static GlobalTransform,
        &'static RenderTarget,
    ),
    With<WorldCamera>,
>;

#[allow(clippy::too_many_arguments)]
fn sight_hits(
    pointers: Query<'_, '_, (&PointerId, &PointerLocation)>,
    windows: Query<'_, '_, Entity, With<PrimaryWindow>>,
    cameras: Cameras<'_, '_>,
    world: Query<'_, '_, Entity, With<OnTheWorld>>,
    sight: Sight<'_, '_>,
    settings: Res<'_, SightPicking>,
    mut pointed: ResMut<'_, Pointed>,
    mut hits: MessageWriter<'_, PointerHits>,
) {
    pointed.0.clear();
    let Ok(world) = world.single() else {
        return;
    };
    let window = windows.single().ok();
    for (&pointer, location) in &pointers {
        let Some(location) = location.location() else {
            continue;
        };
        for (camera_entity, camera, placed, target) in &cameras {
            let on_target = target.normalize(window).as_ref() == Some(&location.target);
            if !camera.is_active || !on_target {
                continue;
            }
            let Ok(ray) = camera.viewport_to_world(placed, location.position) else {
                continue;
            };
            let depth_left =
                (FARCLIP - NEARCLIP) / ray.direction.dot(*placed.forward()).max(f32::EPSILON);
            let cast = sight.cast(ray.origin, ray.direction, depth_left);
            let under = cast.ground();
            let first = match *settings {
                SightPicking::Models => cast.first(),
                SightPicking::ThroughToTheGround => under.clone(),
            };
            let picks = first
                .iter()
                .map(|s| {
                    let data = HitData::new(camera_entity, s.distance, Some(s.point), None);
                    (world, data)
                })
                .collect();
            hits.write(PointerHits::new(pointer, picks, camera.order as f32));
            pointed.0.push((
                pointer,
                PointedAt {
                    first,
                    ground: under,
                },
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::uuid::Uuid;
    use bevy::picking::{InteractionPlugin, PickingPlugin};

    use super::*;

    const POINTER: PointerId = PointerId::Custom(Uuid::from_u128(7));

    fn hovered(panel_order: Option<f32>) -> Option<PointedAt> {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PickingPlugin, InteractionPlugin));
        let world = app.world_mut().spawn(OnTheWorld).id();
        let panel = app.world_mut().spawn_empty().id();
        let camera = app.world_mut().spawn_empty().id();
        app.world_mut().spawn(POINTER);
        app.add_systems(
            PreUpdate,
            (move |mut hits: MessageWriter<'_, PointerHits>| {
                let at = |e: Entity, depth: f32| (e, HitData::new(camera, depth, None, None));
                hits.write(PointerHits::new(POINTER, vec![at(world, 20.0)], 0.0));
                if let Some(order) = panel_order {
                    hits.write(PointerHits::new(POINTER, vec![at(panel, 0.0)], order));
                }
            })
            .in_set(PickingSystems::Backend),
        );
        app.update();
        let pointed = Pointed(vec![(POINTER, PointedAt::default())]);
        let hover = app.world().resource::<HoverMap>();
        pointed.over_world(POINTER, hover, world).cloned()
    }

    #[test]
    fn a_panel_over_the_world_takes_the_pointer_first() {
        assert!(hovered(None).is_some(), "alone, the world has the pointer");
        assert!(
            hovered(Some(1.0)).is_none(),
            "a panel of a higher order takes it"
        );
        assert!(
            hovered(Some(-1.0)).is_some(),
            "one under the world does not"
        );
    }
}
