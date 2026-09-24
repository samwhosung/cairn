//! Where the liquid is, and whose it is. A liquid is a grid, not a plane: a point is wet when its
//! cell is, and the surface there is the bilinear of the cell's four corners, as the client samples
//! it. Inside a building only that building's own liquid answers, outdoors only the terrain's.

use bevy::prelude::*;
use light::Submersion;
use terrain::{LiquidKind, LiquidMesh};

use crate::coords::{bevy_to_wow, wow_to_bevy};
use crate::interior::WmoRoom;
use crate::wmo::{WmoGroupNav, WmoRooms};

/// A query landing this far outside the grid, in cells, still counts as on it.
const GRID_EDGE_TOLERANCE: f32 = 1e-3;

/// How far over a water surface the eye still reads as under it. The client's magma and slime
/// test has no margin.
const SUBMERSION_EPS: f32 = 0.01;

/// A WMO pool's scope: the room it belongs to, and the lowest point of that room's box in world
/// Z. Nothing below a pool's room is in its liquid.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WmoPool {
    /// `None` for a building no subject can be inside of.
    pub(crate) owner: Option<WmoRoom>,
    pub(crate) floor: f32,
}

impl WmoPool {
    /// Group `group`'s pool in a building placed by `transform`. The building's room owns it when
    /// the building has portals or an area id; otherwise no subject is ever inside it.
    pub(crate) fn of(
        rooms: &WmoRooms,
        group: usize,
        instance: Entity,
        transform: &Transform,
    ) -> Self {
        let owned = rooms.has_portals() || rooms.wmo_id != 0;
        let room = WmoRoom {
            instance,
            group: group as u16,
        };
        Self::new(owned.then_some(room), transform, rooms.group_nav.get(group))
    }

    /// The floor is the group box's lowest corner under the placement; with no box, none.
    pub(crate) fn new(
        owner: Option<WmoRoom>,
        transform: &Transform,
        nav: Option<&WmoGroupNav>,
    ) -> Self {
        let Some(g) = nav.filter(|g| g.bbox_min[0] <= g.bbox_max[0]) else {
            return Self {
                owner,
                floor: f32::NEG_INFINITY,
            };
        };
        let mut floor = f32::INFINITY;
        for x in [g.bbox_min[0], g.bbox_max[0]] {
            for y in [g.bbox_min[1], g.bbox_max[1]] {
                for z in [g.bbox_min[2], g.bbox_max[2]] {
                    floor = floor.min(transform.transform_point(wow_to_bevy([x, y, z])).y);
                }
            }
        }
        Self { owner, floor }
    }
}

/// Which file a liquid surface came from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LiquidSource {
    /// A terrain chunk's `MCLQ`.
    AdtChunk,
    /// A building group's `MLIQ`.
    WmoGroup(WmoPool),
}

/// Whose liquid answers for a subject.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LiquidClaim {
    /// In the open world: the terrain's liquid.
    Outdoors,
    /// In a building: that building's liquid alone.
    Inside {
        room: WmoRoom,
        /// The group is wholly under this liquid, at any height and with no surface.
        flooded: Option<LiquidKind>,
    },
    /// Not yet known: both answer.
    Unknown,
}

impl LiquidClaim {
    /// A subject in `room` of a building whose groups `nav` describes.
    pub fn inside(room: WmoRoom, nav: &[WmoGroupNav]) -> Self {
        Self::Inside {
            room,
            flooded: nav.get(usize::from(room.group)).and_then(|g| g.flooded),
        }
    }
}

/// One liquid surface in world WoW space, with its wet cells and the lattice's basis.
#[derive(Component)]
pub struct LiquidGrid {
    min: [f32; 2],
    max: [f32; 2],
    source: LiquidSource,
    kind: LiquidKind,
    cols: usize,
    rows: usize,
    positions: Vec<[f32; 3]>,
    wet: Vec<bool>,
    origin: [f32; 2],
    /// World XY per step in `i` and in `j`, over the full span to keep f32 error at one ulp.
    u: [f32; 2],
    v: [f32; 2],
    /// `None` when the grid is degenerate in XY; queries then fall back to the box.
    inv_det: Option<f32>,
    /// The highest wet vertex: the degenerate grid's answer, and a dry cell's nearest point's.
    fallback_z: f32,
    sound_nibble: u8,
}

impl LiquidGrid {
    /// From a world-space grid of `cols × rows` vertices and one wet flag per cell. A grid whose
    /// arrays do not match its size claims nothing.
    pub fn new(
        source: LiquidSource,
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
            source,
            kind,
            cols: 0,
            rows: 0,
            positions: Vec::new(),
            wet: Vec::new(),
            origin: [0.0; 2],
            u: [0.0; 2],
            v: [0.0; 2],
            inv_det: None,
            fallback_z: f32::MIN,
            sound_nibble: 0,
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
        out.rows = rows;
        out.positions = positions;
        out.wet = wet;
        out
    }

    pub fn kind(&self) -> LiquidKind {
        self.kind
    }

    pub(crate) fn sound_nibble(&self) -> u8 {
        self.sound_nibble
    }

    /// The point of the wet footprint's box nearest a WoW XY, on the surface there, or at the
    /// highest wet vertex where that lands over a dry cell; `None` for a grid with no wet cell.
    pub(crate) fn nearest_point(&self, x: f32, y: f32) -> Option<[f32; 3]> {
        if self.min[0] > self.max[0] {
            return None;
        }
        let cx = x.clamp(self.min[0], self.max[0]);
        let cy = y.clamp(self.min[1], self.max[1]);
        Some([cx, cy, self.surface_z_at(cx, cy).unwrap_or(self.fallback_z)])
    }

    /// The highest wet vertex, which a surface once answered from anywhere over its box.
    #[cfg(test)]
    pub(super) fn highest_wet_z(&self) -> f32 {
        self.fallback_z
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

    pub(crate) fn contains(&self, x: f32, y: f32) -> bool {
        (self.min[0]..=self.max[0]).contains(&x) && (self.min[1]..=self.max[1]).contains(&y)
    }

    pub(crate) fn overlaps(&self, lo: [f32; 2], hi: [f32; 2]) -> bool {
        hi[0] >= self.min[0] && lo[0] <= self.max[0] && hi[1] >= self.min[1] && lo[1] <= self.max[1]
    }

    /// Calls `f` with every wet cell's four corners, `[tl, tr, bl, br]`, in world WoW space.
    pub(crate) fn for_each_wet_cell(&self, mut f: impl FnMut([[f32; 3]; 4])) {
        let Some(cells_x) = self.cols.checked_sub(1) else {
            return;
        };
        for cell in (0..self.wet.len()).filter(|&c| self.wet[c]) {
            let (i, j) = (cell % cells_x, cell / cells_x);
            let p = |di: usize, dj: usize| self.positions[(j + dj) * self.cols + i + di];
            f([p(0, 0), p(1, 0), p(0, 1), p(1, 1)]);
        }
    }

    /// `[[min_x, min_y], [max_x, max_y]]` of the wet cells; `None` when none is.
    pub(crate) fn xy_bounds(&self) -> Option<[[f32; 2]; 2]> {
        (self.min[0] <= self.max[0] && self.min[1] <= self.max[1]).then_some([self.min, self.max])
    }

    /// Whether this surface answers for a subject holding `claim` whose WoW height is `z`.
    fn answers(&self, claim: LiquidClaim, z: f32) -> bool {
        match (claim, self.source) {
            (_, LiquidSource::WmoGroup(pool)) if z < pool.floor => false,
            (LiquidClaim::Unknown, _) | (LiquidClaim::Outdoors, LiquidSource::AdtChunk) => true,
            (LiquidClaim::Outdoors, LiquidSource::WmoGroup(_))
            | (LiquidClaim::Inside { .. }, LiquidSource::AdtChunk) => false,
            (LiquidClaim::Inside { room, .. }, LiquidSource::WmoGroup(pool)) => {
                pool.owner.is_some_and(|o| o.instance == room.instance)
            }
        }
    }

    fn wet_cell_at(&self, x: f32, y: f32) -> Option<(usize, usize, f32, f32)> {
        let (cells_x, cells_y) = (self.cols.checked_sub(1)?, self.rows.checked_sub(1)?);
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
}

/// A liquid surface's grid in world WoW space, `transform` carrying its mesh into the world.
pub fn wet_footprint(mesh: &LiquidMesh, transform: &Transform, source: LiquidSource) -> LiquidGrid {
    let positions = mesh
        .positions
        .iter()
        .map(|&p| bevy_to_wow(transform.transform_point(wow_to_bevy(p))))
        .collect();
    let mut grid = LiquidGrid::new(
        source,
        mesh.kind,
        [mesh.grid[0] as usize, mesh.grid[1] as usize],
        positions,
        mesh.wet.clone(),
    );
    grid.sound_nibble = mesh.sound_nibble;
    grid
}

/// One liquid a query landed in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LiquidHit {
    /// WoW Z.
    pub surface_z: f32,
    pub kind: LiquidKind,
}

/// The liquid over a WoW position's column for a subject holding `claim`; where surfaces stack,
/// the lowest. A flooded room is the answer on its own, at `f32::MAX`.
pub fn liquid_at<'a>(
    grids: impl Iterator<Item = &'a LiquidGrid>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<LiquidHit> {
    if let LiquidClaim::Inside {
        flooded: Some(kind),
        ..
    } = claim
    {
        return Some(LiquidHit {
            surface_z: f32::MAX,
            kind,
        });
    }
    grids
        .filter(|g| g.answers(claim, wow[2]))
        .filter_map(|g| {
            g.surface_z_at(wow[0], wow[1]).map(|surface_z| LiquidHit {
                surface_z,
                kind: g.kind,
            })
        })
        .min_by(|a, b| a.surface_z.total_cmp(&b.surface_z))
}

/// [`liquid_at`] over water alone: magma and slime are swum in but splash nothing.
pub fn water_surface_at<'a>(
    grids: impl Iterator<Item = &'a LiquidGrid>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<f32> {
    liquid_at(grids.filter(|g| !g.kind.is_fullbright()), wow, claim).map(|h| h.surface_z)
}

/// Every surface height over a WoW position's column that answers for `claim`.
pub fn surfaces_at<'a>(
    grids: impl Iterator<Item = &'a LiquidGrid> + 'a,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> impl Iterator<Item = f32> + 'a {
    grids
        .filter(move |g| g.answers(claim, wow[2]))
        .filter_map(move |g| g.surface_z_at(wow[0], wow[1]))
}

fn submersion_of(kind: LiquidKind) -> Submersion {
    match kind {
        LiquidKind::Still | LiquidKind::Rapids => Submersion::Water,
        LiquidKind::Ocean => Submersion::Ocean,
        LiquidKind::Magma => Submersion::Magma,
        LiquidKind::Slime => Submersion::Slime,
    }
}

/// What a point is submerged in, and the surface over it: of every answering surface the point
/// is under, the lowest.
pub fn submersion_claim_at<'a>(
    grids: impl Iterator<Item = &'a LiquidGrid>,
    wow: [f32; 3],
    claim: LiquidClaim,
) -> Option<(Submersion, f32)> {
    grids
        .filter(|g| g.answers(claim, wow[2]))
        .filter_map(|g| {
            let z = g.surface_z_at(wow[0], wow[1])?;
            let eps = if g.kind.is_fullbright() {
                0.0
            } else {
                SUBMERSION_EPS
            };
            (wow[2] < z + eps).then_some((z, submersion_of(g.kind)))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(z, s)| (s, z))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests;
