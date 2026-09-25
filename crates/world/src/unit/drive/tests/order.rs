use bevy::animation::transition::AnimationTransitions;
use bevy::prelude::*;

use super::{STAND, app, body, frames};
use crate::rig::ModelAnimations;

fn stand_rolled(app: &App, unit: Entity) -> Option<usize> {
    let e = app.world().entity(unit);
    let node = e
        .get::<AnimationTransitions>()
        .and_then(AnimationTransitions::get_main_animation)?;
    let anims = e.get::<ModelAnimations>()?;
    anims
        .clips
        .iter()
        .find(|c| c.node == node)
        .map(|c| c.seq_index)
}

fn rolled_by_the_bodies_west_and_east(west_spawned_first: bool) -> [Option<usize>; 2] {
    let mut app = app();
    let rows = [
        (STAND, 1.0, true, 0.0, 0x1000, (1, 1)),
        (STAND, 1.0, true, 0.0, 0x7000, (1, 1)),
    ];
    let mut stood_at = |x: f32| {
        let unit = body(&mut app, &rows);
        app.world_mut()
            .entity_mut(unit)
            .insert(Transform::from_xyz(x, 0.0, 0.0));
        unit
    };
    let (west, east) = if west_spawned_first {
        let west = stood_at(-3.0);
        (west, stood_at(3.0))
    } else {
        let east = stood_at(3.0);
        (stood_at(-3.0), east)
    };
    frames(&mut app, 1);
    [west, east].map(|unit| stand_rolled(&app, unit))
}

#[test]
fn bodies_rolling_in_one_frame_take_the_stream_in_the_order_they_stand() {
    let west_first = rolled_by_the_bodies_west_and_east(true);
    let east_first = rolled_by_the_bodies_west_and_east(false);
    assert_eq!(west_first, [Some(0), Some(1)]);
    assert_eq!(west_first, east_first, "the rolls follow the spawn order");
}
