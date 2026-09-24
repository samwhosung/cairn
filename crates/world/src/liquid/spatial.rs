use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use super::query::LiquidGrid;

const CELL: f32 = terrain::CHUNK_SIZE;

fn cell_of(x: f32, y: f32) -> [i32; 2] {
    [(x / CELL).floor() as i32, (y / CELL).floor() as i32]
}

#[derive(Default)]
pub struct SpatialIndex {
    cells: HashMap<[i32; 2], Vec<Entity>>,
}

impl SpatialIndex {
    pub fn rebuild<'a>(&mut self, grids: impl Iterator<Item = (Entity, &'a LiquidGrid)>) {
        self.cells.clear();
        for (entity, grid) in grids {
            let Some(bounds) = grid.xy_bounds() else {
                continue;
            };
            let [x0, y0] = cell_of(bounds.min.x, bounds.min.y);
            let [x1, y1] = cell_of(bounds.max.x, bounds.max.y);
            for cx in x0..=x1 {
                for cy in y0..=y1 {
                    self.cells.entry([cx, cy]).or_default().push(entity);
                }
            }
        }
    }

    /// Every surface whose wet box overlaps the cell holding this WoW XY.
    pub fn over(&self, x: f32, y: f32) -> &[Entity] {
        self.cells.get(&cell_of(x, y)).map_or(&[], Vec::as_slice)
    }

    /// Every surface in any cell the WoW box `[lo, hi]` touches, once each.
    pub fn over_box(&self, lo: [f32; 2], hi: [f32; 2]) -> Vec<Entity> {
        let ([x0, y0], [x1, y1]) = (cell_of(lo[0], lo[1]), cell_of(hi[0], hi[1]));
        let mut out = Vec::new();
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                for &e in self.cells.get(&[cx, cy]).map_or(&[][..], Vec::as_slice) {
                    if !out.contains(&e) {
                        out.push(e);
                    }
                }
            }
        }
        out
    }
}

/// The index over every drawn liquid surface.
#[derive(Resource, Default)]
pub struct WaterIndex(pub SpatialIndex);

pub(super) fn maintain_water_index(
    mut index: ResMut<'_, WaterIndex>,
    added: Query<'_, '_, (), Added<LiquidGrid>>,
    mut removed: RemovedComponents<'_, '_, LiquidGrid>,
    grids: Query<'_, '_, (Entity, &LiquidGrid)>,
) {
    if removed.read().next().is_none() && added.is_empty() {
        return;
    }
    index.0.rebuild(grids.iter());
}

#[cfg(test)]
mod tests {
    use terrain::LiquidKind;

    use super::super::query::{LiquidClaim, LiquidSource, surfaces_at};
    use super::*;

    fn chunk(x0: f32, y0: f32, z: f32, n: usize) -> LiquidGrid {
        let positions = (0..n)
            .flat_map(|j| (0..n).map(move |i| [x0 + 10.0 * i as f32, y0 + 10.0 * j as f32, z]))
            .collect();
        LiquidGrid::new(
            LiquidSource::AdtChunk,
            LiquidKind::Still,
            [n, n],
            positions,
            vec![true; (n - 1) * (n - 1)],
        )
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<WaterIndex>()
            .add_systems(Update, maintain_water_index);
        app
    }

    #[test]
    fn the_index_answers_as_the_full_walk_does() {
        let mut app = app();
        app.world_mut().spawn(chunk(-20.0, -20.0, 5.0, 3));
        app.world_mut().spawn(chunk(-50.0, -50.0, 8.0, 12));
        app.update();
        let world = app.world_mut();
        let index = world.remove_resource::<WaterIndex>().expect("the index");
        let mut grids = world.query::<&LiquidGrid>();
        for x in (-60..=70).step_by(7) {
            for y in (-60..=70).step_by(7) {
                let wow = [x as f32, y as f32, 0.0];
                let mut walk: Vec<f32> =
                    surfaces_at(grids.iter(world), wow, LiquidClaim::Outdoors).collect();
                let near: Vec<&LiquidGrid> = index
                    .0
                    .over(wow[0], wow[1])
                    .iter()
                    .filter_map(|&e| grids.get(world, e).ok())
                    .collect();
                let mut indexed: Vec<f32> =
                    surfaces_at(near.into_iter(), wow, LiquidClaim::Outdoors).collect();
                walk.sort_by(f32::total_cmp);
                indexed.sort_by(f32::total_cmp);
                assert_eq!(walk, indexed, "at ({x}, {y})");
            }
        }
    }

    #[test]
    fn a_despawned_surface_leaves_the_index() {
        let mut app = app();
        let e = app.world_mut().spawn(chunk(0.0, 0.0, 5.0, 3)).id();
        app.update();
        assert!(
            !app.world()
                .resource::<WaterIndex>()
                .0
                .over(5.0, 5.0)
                .is_empty()
        );
        app.world_mut().entity_mut(e).despawn();
        app.update();
        assert!(
            app.world()
                .resource::<WaterIndex>()
                .0
                .over(5.0, 5.0)
                .is_empty()
        );
    }
}
