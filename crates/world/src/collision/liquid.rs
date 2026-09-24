//! Where the liquid is: every streamed liquid surface's grid, and the query swimming asks of it.
//! A liquid is a grid, not a plane: a point is wet when its cell is, and the surface there is the
//! bilinear of the cell's four corner heights, as the client samples it.

use avian3d::prelude::{Collider, RigidBody};
use bevy::ecs::system::SystemParam;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use terrain::{LiquidKind, LiquidMesh};

use crate::coords::wow_to_bevy;

/// A query landing this far outside the grid, in cells, still counts as on it.
const GRID_EDGE_TOLERANCE: f32 = 1e-3;

/// One liquid surface in world WoW space, with its wet cells and the lattice's basis.
#[derive(Component)]
pub struct LiquidSurface {
    min: [f32; 2],
    max: [f32; 2],
    kind: LiquidKind,
    /// The sound class the liquid's loop is looked up by: class `n & 3`, speed `n & 0xc`.
    sound_nibble: u8,
    cols: usize,
    positions: Vec<[f32; 3]>,
    wet: Vec<bool>,
    origin: [f32; 2],
    /// World XY per step in `i` and in `j`, over the full span to keep f32 error at one ulp.
    u: [f32; 2],
    v: [f32; 2],
    /// `None` when the grid is degenerate in XY; queries then fall back to the box.
    inv_det: Option<f32>,
    /// The highest wet vertex: the degenerate grid's answer only.
    fallback_z: f32,
}

impl LiquidSurface {
    /// From a world-space grid of `cols × rows` vertices and one wet flag per cell. A grid whose
    /// arrays do not match its size claims nothing.
    pub fn new(
        kind: LiquidKind,
        [cols, rows]: [usize; 2],
        positions: Vec<[f32; 3]>,
        wet: Vec<bool>,
    ) -> Self {
        let sane = cols >= 2
            && rows >= 2
            && positions.len() == cols * rows
            && wet.len() == (cols - 1) * (rows - 1);
        let mut out = Self {
            min: [f32::MAX; 2],
            max: [f32::MIN; 2],
            kind,
            sound_nibble: 0,
            cols: 0,
            positions: Vec::new(),
            wet: Vec::new(),
            origin: [0.0; 2],
            u: [0.0; 2],
            v: [0.0; 2],
            inv_det: None,
            fallback_z: f32::MIN,
        };
        if !sane {
            return out;
        }
        for cell in (0..wet.len()).filter(|&c| wet[c]) {
            let (i, j) = (cell % (cols - 1), cell / (cols - 1));
            for (di, dj) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = positions[(j + dj) * cols + i + di];
                out.min = [out.min[0].min(p[0]), out.min[1].min(p[1])];
                out.max = [out.max[0].max(p[0]), out.max[1].max(p[1])];
                out.fallback_z = out.fallback_z.max(p[2]);
            }
        }
        let origin = [positions[0][0], positions[0][1]];
        let step = |far: [f32; 3], n: usize| {
            [
                (far[0] - origin[0]) / n as f32,
                (far[1] - origin[1]) / n as f32,
            ]
        };
        out.u = step(positions[cols - 1], cols - 1);
        out.v = step(positions[(rows - 1) * cols], rows - 1);
        let det = out.u[0] * out.v[1] - out.u[1] * out.v[0];
        out.inv_det = (det.abs() > 1e-9).then(|| 1.0 / det);
        out.origin = origin;
        out.cols = cols;
        out.positions = positions;
        out.wet = wet;
        out
    }

    /// An ADT liquid block's surface; its positions are already world WoW.
    pub fn from_mesh(mesh: &LiquidMesh) -> Self {
        let mut surface = Self::new(
            mesh.kind,
            [mesh.grid[0] as usize, mesh.grid[1] as usize],
            mesh.positions.clone(),
            mesh.wet.clone(),
        );
        surface.sound_nibble = mesh.sound_nibble;
        surface
    }

    /// The point of the wet footprint's box nearest a WoW XY, on the surface there, or at the
    /// highest wet vertex where that lands over a dry cell.
    pub fn nearest_point_wow(&self, x: f32, y: f32) -> [f32; 3] {
        let cx = x.clamp(self.min[0], self.max[0]);
        let cy = y.clamp(self.min[1], self.max[1]);
        [cx, cy, self.surface_z_at(cx, cy).unwrap_or(self.fallback_z)]
    }

    /// The surface height (WoW Z) at a WoW XY, or `None` where this liquid is not.
    pub fn surface_z_at(&self, x: f32, y: f32) -> Option<f32> {
        if !self.contains(x, y) {
            return None;
        }
        match self.wet_cell_at(x, y) {
            Some((i, j, fx, fy)) => Some(self.height_in_cell(i, j, fx, fy)),
            None if self.inv_det.is_none() => Some(self.fallback_z),
            None => None,
        }
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        (self.min[0]..=self.max[0]).contains(&x) && (self.min[1]..=self.max[1]).contains(&y)
    }

    fn wet_cell_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32, f32)> {
        let cells_x = self.cols.checked_sub(1)?;
        let cells_y = (self.positions.len() / self.cols.max(1)).checked_sub(1)?;
        let inv_det = self.inv_det?;
        let (dx, dy) = (x - self.origin[0], y - self.origin[1]);
        let a = (dx * self.v[1] - dy * self.v[0]) * inv_det;
        let b = (self.u[0] * dy - self.u[1] * dx) * inv_det;
        let snap = |t: f32, cells: usize| -> Option<(usize, f32)> {
            if t < -GRID_EDGE_TOLERANCE || t > cells as f32 + GRID_EDGE_TOLERANCE {
                return None;
            }
            let idx = (t.floor().max(0.0) as usize).min(cells - 1);
            Some((idx, (t - idx as f32).clamp(0.0, 1.0)))
        };
        let (i, fx) = snap(a, cells_x)?;
        let (j, fy) = snap(b, cells_y)?;
        self.wet.get(j * cells_x + i)?.then_some((i, j, fx, fy))
    }

    fn height_in_cell(&self, i: usize, j: usize, fx: f32, fy: f32) -> f32 {
        let z = |i: usize, j: usize| self.positions[j * self.cols + i][2];
        let t1 = z(i, j) + (z(i + 1, j) - z(i, j)) * fx;
        let t2 = z(i, j + 1) + (z(i + 1, j + 1) - z(i, j + 1)) * fx;
        t1 + (t2 - t1) * fy
    }

    fn xy_bounds(&self) -> Option<[[f32; 2]; 2]> {
        (self.min[0] <= self.max[0] && self.min[1] <= self.max[1]).then_some([self.min, self.max])
    }
}

/// The waterline the camera may hit: the surface's wet triangles. `None` when none is wet.
pub(super) fn liquid_collider(mesh: &LiquidMesh) -> Option<(Collider, RigidBody)> {
    let tris: Vec<[u32; 3]> = mesh
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect();
    if tris.is_empty() {
        return None;
    }
    let verts = mesh.positions.iter().map(|p| wow_to_bevy(*p)).collect();
    Some((Collider::trimesh(verts, tris), RigidBody::Static))
}

/// Which surfaces answer for a subject: outdoors the terrain's, inside a building only that
/// building's, and before a subject's room is known, both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiquidClaim {
    Outdoors,
    Inside,
    Unknown,
}

impl LiquidClaim {
    /// Every surface here is the terrain's.
    fn admits_terrain(self) -> bool {
        self != LiquidClaim::Inside
    }
}

/// The nearest wet point of one liquid sound class.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct NearestLiquid {
    pub dist_sq: f32,
    /// WoW space.
    pub point: [f32; 3],
    pub nibble: u8,
}

/// One liquid a query landed in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LiquidHit {
    /// WoW Z.
    pub surface_z: f32,
    pub kind: LiquidKind,
}

/// A one-chunk grid hash over every surface's box, rebuilt when surfaces come and go.
#[derive(Resource, Default)]
pub(crate) struct WaterIndex {
    cells: HashMap<[i32; 2], Vec<Entity>>,
}

const CELL: f32 = 100.0 / 3.0;

fn cell_of(x: f32, y: f32) -> [i32; 2] {
    [(x / CELL).floor() as i32, (y / CELL).floor() as i32]
}

pub(super) fn maintain_water_index(
    mut index: ResMut<'_, WaterIndex>,
    added: Query<'_, '_, (), Added<LiquidSurface>>,
    mut removed: RemovedComponents<'_, '_, LiquidSurface>,
    surfaces: Query<'_, '_, (Entity, &LiquidSurface)>,
) {
    if removed.read().next().is_none() && added.is_empty() {
        return;
    }
    index.cells.clear();
    for (entity, surface) in &surfaces {
        let Some([lo, hi]) = surface.xy_bounds() else {
            continue;
        };
        let ([x0, y0], [x1, y1]) = (cell_of(lo[0], lo[1]), cell_of(hi[0], hi[1]));
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                index.cells.entry([cx, cy]).or_default().push(entity);
            }
        }
    }
}

/// The liquid under a point, from the streamed surfaces.
#[derive(SystemParam)]
pub struct Liquids<'w, 's> {
    index: Res<'w, WaterIndex>,
    surfaces: Query<'w, 's, &'static LiquidSurface>,
}

impl Liquids<'_, '_> {
    /// The liquid over a WoW-space position's column; where surfaces overlap, the lowest.
    pub fn liquid_at(&self, wow: [f32; 3]) -> Option<LiquidHit> {
        self.index
            .cells
            .get(&cell_of(wow[0], wow[1]))?
            .iter()
            .filter_map(|&e| self.surfaces.get(e).ok())
            .filter_map(|s| {
                s.surface_z_at(wow[0], wow[1]).map(|surface_z| LiquidHit {
                    surface_z,
                    kind: s.kind,
                })
            })
            .min_by(|a, b| a.surface_z.total_cmp(&b.surface_z))
    }

    /// The water surface over a WoW position for a subject holding `claim`, lava and slime left
    /// out; where waters overlap, the lowest.
    pub fn water_surface_at(&self, wow: [f32; 3], claim: LiquidClaim) -> Option<f32> {
        if !claim.admits_terrain() {
            return None;
        }
        self.index
            .cells
            .get(&cell_of(wow[0], wow[1]))?
            .iter()
            .filter_map(|&e| self.surfaces.get(e).ok())
            .filter(|s| !s.kind.is_fullbright())
            .filter_map(|s| s.surface_z_at(wow[0], wow[1]))
            .min_by(f32::total_cmp)
    }

    /// Every liquid surface whose footprint covers a WoW XY, with its kind and height there, for
    /// a subject holding `claim`.
    pub fn surfaces_at(&self, wow: [f32; 3], claim: LiquidClaim) -> Vec<LiquidHit> {
        if !claim.admits_terrain() {
            return Vec::new();
        }
        self.index
            .cells
            .get(&cell_of(wow[0], wow[1]))
            .into_iter()
            .flatten()
            .filter_map(|&e| self.surfaces.get(e).ok())
            .filter_map(|s| {
                s.surface_z_at(wow[0], wow[1]).map(|surface_z| LiquidHit {
                    surface_z,
                    kind: s.kind,
                })
            })
            .collect()
    }

    /// The nearest wet point within `radius` of a WoW position, one per sound class `nibble & 3`.
    pub fn nearest_per_class(&self, wow: [f32; 3], radius: f32) -> [Option<NearestLiquid>; 4] {
        let mut best: [Option<NearestLiquid>; 4] = [None; 4];
        for s in &self.surfaces {
            let point = s.nearest_point_wow(wow[0], wow[1]);
            let dist_sq = (point[0] - wow[0]).powi(2)
                + (point[1] - wow[1]).powi(2)
                + (point[2] - wow[2]).powi(2);
            let class = usize::from(s.sound_nibble & 3);
            if dist_sq <= radius * radius && best[class].is_none_or(|b| dist_sq < b.dist_sq) {
                best[class] = Some(NearestLiquid {
                    dist_sq,
                    point,
                    nibble: s.sound_nibble,
                });
            }
        }
        best
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(
        x0: f32,
        y0: f32,
        cols: usize,
        rows: usize,
        z: impl Fn(usize, usize) -> f32,
    ) -> Vec<[f32; 3]> {
        (0..rows)
            .flat_map(|j| (0..cols).map(move |i| (i, j)))
            .map(|(i, j)| [x0 + 10.0 * i as f32, y0 + 10.0 * j as f32, z(i, j)])
            .collect()
    }

    #[test]
    fn the_surface_is_the_bilinear_of_its_cell_not_the_grid_maximum() {
        let s = LiquidSurface::new(
            LiquidKind::Still,
            [3, 3],
            grid(0.0, 0.0, 3, 3, |i, _| i as f32 * 2.0),
            vec![true; 4],
        );
        let z = s.surface_z_at(5.0, 5.0).expect("wet");
        assert!((z - 1.0).abs() < 1e-5, "{z}");
        assert!((s.surface_z_at(15.0, 12.0).expect("wet") - 3.0).abs() < 1e-5);
        assert_eq!(s.surface_z_at(25.0, 5.0), None, "outside the box");
    }

    #[test]
    fn a_dry_cell_inside_the_box_is_dry() {
        let s = LiquidSurface::new(
            LiquidKind::Ocean,
            [3, 3],
            grid(0.0, 0.0, 3, 3, |_, _| 7.0),
            vec![true, false, true, true],
        );
        assert_eq!(s.surface_z_at(15.0, 5.0), None);
        assert_eq!(s.surface_z_at(5.0, 5.0), Some(7.0));
    }

    #[test]
    fn a_grid_whose_arrays_disagree_claims_nothing() {
        let s = LiquidSurface::new(
            LiquidKind::Still,
            [3, 3],
            grid(0.0, 0.0, 2, 2, |_, _| 0.0),
            vec![true],
        );
        assert_eq!(s.surface_z_at(1.0, 1.0), None);
        assert!(s.xy_bounds().is_none());
    }

    #[test]
    fn a_rotated_grid_inverts_to_its_cell() {
        let (c, sn) = (0.3f32.cos(), 0.3f32.sin());
        let positions = (0..3)
            .flat_map(|j| (0..3).map(move |i| (i as f32 * 4.0, j as f32 * 4.0)))
            .map(|(a, b)| [100.0 + a * c - b * sn, 50.0 + a * sn + b * c, a])
            .collect();
        let s = LiquidSurface::new(LiquidKind::Magma, [3, 3], positions, vec![true; 4]);
        let (a, b) = (6.0f32, 2.0f32);
        let (x, y) = (100.0 + a * c - b * sn, 50.0 + a * sn + b * c);
        assert!((s.surface_z_at(x, y).expect("wet") - 6.0).abs() < 1e-3);
    }
}
