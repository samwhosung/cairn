//! The editor's hands on the world: what a pointer points at, the placements shown selected, and a
//! ghost of one about to be placed.

mod ghost;
mod marks;
mod pick;
mod select;

use std::collections::BTreeMap;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibilitySystems;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

pub use ghost::{Ghost, GhostShown};
pub use pick::{
    OnTheWorld, PICKS_FRAMES_LATE, Pointed, PointedAt, SightPicking, SightPickingPlugin,
};
pub use select::Selection;

/// Where the selected placements and the ghost lie on the world camera's frame, in its logical
/// pixels from the top left, as last drawn: each selected placement by the box its corners are
/// drawn at, and the ghost by the box its model and footprint reach.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct OnScreen {
    pub selected: BTreeMap<u32, Rect>,
    pub ghost: Option<Rect>,
}

fn reach_on_screen(
    (camera, eye): (&Camera, &GlobalTransform),
    at: &GlobalTransform,
    bound: &Aabb,
) -> Option<Rect> {
    let (lo, hi) = (Vec3::from(bound.min()), Vec3::from(bound.max()));
    (0..8)
        .filter_map(|k| {
            let corner = Vec3::new(
                if k & 1 == 0 { lo.x } else { hi.x },
                if k & 2 == 0 { lo.y } else { hi.y },
                if k & 4 == 0 { lo.z } else { hi.z },
            );
            camera
                .world_to_viewport(eye, at.transform_point(corner))
                .ok()
        })
        .map(|px| Rect::from_center_size(px, Vec2::ZERO))
        .reduce(|a, b| a.union(b))
}

/// Draws the [`Selection`] and the [`Ghost`]. Needs the [`WorldPlugin`](crate::WorldPlugin); a
/// ghost follows a pointer through the [`SightPickingPlugin`].
pub struct HandsPlugin;

impl Plugin for HandsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(marks::MarksPlugin)
            .init_resource::<Selection>()
            .init_resource::<OnScreen>()
            .init_resource::<select::SelectedLook>()
            .init_resource::<Ghost>()
            .init_resource::<GhostShown>()
            .init_resource::<ghost::GhostDrawn>()
            .add_systems(Startup, select::spawn_corners)
            .add_systems(
                Update,
                (ghost::follow_pointer, ghost::draw_ghost)
                    .chain()
                    .after(crate::WorldSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    select::tint_selected.before(TransformSystems::Propagate),
                    select::corner_selected
                        .after(TransformSystems::Propagate)
                        .before(VisibilitySystems::CheckVisibility),
                    ghost::ghost_on_screen.after(TransformSystems::Propagate),
                ),
            );
    }
}
