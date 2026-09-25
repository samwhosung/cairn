use std::hash::{Hash, Hasher};

use libm::{cosf, sinf, sqrtf};

const CELL_YD: f32 = 16.0;

/// Where a body stands and which way it faces: world yards, x north, y west, z up, and radians
/// counterclockwise from north. Two spots are equal when their bits are.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spot {
    pub pos: [f32; 3],
    pub facing: f32,
}

impl Spot {
    fn bits(&self) -> [u32; 4] {
        [
            self.pos[0].to_bits(),
            self.pos[1].to_bits(),
            self.pos[2].to_bits(),
            self.facing.to_bits(),
        ]
    }

    /// How far `to` stands on the ground, whatever its height.
    pub fn across(&self, to: [f32; 3]) -> f32 {
        let (dx, dy) = (to[0] - self.pos[0], to[1] - self.pos[1]);
        sqrtf(dx * dx + dy * dy)
    }

    /// Whether `to` lies within `arc` radians centred on the facing, on the ground; a point
    /// straight above or below lies in every arc.
    pub fn faces(&self, to: [f32; 3], arc: f32) -> bool {
        let (dx, dy) = (to[0] - self.pos[0], to[1] - self.pos[1]);
        let along = dx * cosf(self.facing) + dy * sinf(self.facing);
        along >= cosf(arc / 2.0) * sqrtf(dx * dx + dy * dy)
    }
}

impl PartialEq for Spot {
    fn eq(&self, other: &Self) -> bool {
        self.bits() == other.bits()
    }
}

impl Eq for Spot {}

impl Hash for Spot {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.bits().hash(state);
    }
}

/// The players' bodies as last tick left them, and where each first stood.
#[derive(Default)]
pub struct Space {
    bodies: Vec<Option<Spot>>,
    spawns: Vec<Spot>,
    cells: Vec<(u64, u32)>,
}

fn cell(x: f32, y: f32) -> (i32, i32) {
    ((x / CELL_YD).floor() as i32, (y / CELL_YD).floor() as i32)
}

fn key((cx, cy): (i32, i32)) -> u64 {
    (u64::from(cx as u32) << 32) | u64::from(cy as u32)
}

impl Space {
    pub fn update(&mut self, bodies: &[Option<Spot>]) {
        self.bodies.clear();
        self.bodies.extend_from_slice(bodies);
        self.cells.clear();
        for (n, b) in bodies.iter().enumerate() {
            if let Some(b) = b {
                self.cells.push((key(cell(b.pos[0], b.pos[1])), n as u32));
            }
        }
        self.cells.sort_unstable();
    }

    pub fn join(&mut self, n: u32, spawn: Spot) {
        let n = n as usize;
        if self.spawns.len() <= n {
            self.spawns.resize(n + 1, Spot::default());
        }
        self.spawns[n] = spawn;
    }

    pub fn body(&self, n: u32) -> Option<Spot> {
        self.bodies.get(n as usize).copied().flatten()
    }

    pub fn spawn(&self, n: u32) -> Option<Spot> {
        self.spawns.get(n as usize).copied()
    }

    /// Every present body within `r` of `at` on the ground, in the order of its cell and then its
    /// number.
    pub fn near(&self, at: [f32; 3], r: f32, mut f: impl FnMut(u32, Spot)) {
        let (lo, hi) = (cell(at[0] - r, at[1] - r), cell(at[0] + r, at[1] + r));
        let centre = Spot {
            pos: at,
            facing: 0.0,
        };
        let cells =
            (i64::from(hi.0) - i64::from(lo.0) + 1) * (i64::from(hi.1) - i64::from(lo.1) + 1);
        if cells > self.cells.len() as i64 {
            for &(_, n) in &self.cells {
                if let Some(b) = self.body(n).filter(|b| centre.across(b.pos) <= r) {
                    f(n, b);
                }
            }
            return;
        }
        for cx in lo.0..=hi.0 {
            for cy in lo.1..=hi.1 {
                let k = key((cx, cy));
                let from = self.cells.partition_point(|c| c.0 < k);
                for &(ck, n) in &self.cells[from..] {
                    if ck != k {
                        break;
                    }
                    let Some(b) = self.body(n) else { continue };
                    if centre.across(b.pos) <= r {
                        f(n, b);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::PI;

    use super::*;

    fn spot(x: f32, y: f32, facing: f32) -> Spot {
        Spot {
            pos: [x, y, 0.0],
            facing,
        }
    }

    #[test]
    fn a_query_finds_exactly_the_present_bodies_in_range_across_cell_edges() {
        let bodies: Vec<Option<Spot>> = (0..300)
            .map(|i| {
                (i % 7 != 0).then(|| {
                    spot(
                        (i % 20) as f32 * 3.1 - 30.0,
                        (i / 20) as f32 * 2.7 - 20.0,
                        0.0,
                    )
                })
            })
            .collect();
        let mut space = Space::default();
        space.update(&bodies);
        for (at, r) in [
            ([0.0, 0.0, 0.0], 5.0),
            ([-15.9, 16.1, 3.0], 12.0),
            ([200.0, 0.0, 0.0], 5.0),
        ] {
            let mut got = Vec::new();
            space.near(at, r, |n, _| got.push(n));
            got.sort_unstable();
            let want: Vec<u32> = (0..bodies.len() as u32)
                .filter(|&n| {
                    bodies[n as usize].is_some_and(|b| spot(at[0], at[1], 0.0).across(b.pos) <= r)
                })
                .collect();
            assert_eq!(got, want, "{at:?} {r}");
        }
    }

    #[test]
    fn in_front_is_the_arc_about_the_facing() {
        let north = spot(0.0, 0.0, 0.0);
        assert!(north.faces([5.0, 0.0, 0.0], PI));
        assert!(north.faces([1.0, 4.0, 0.0], PI));
        assert!(!north.faces([-1.0, 4.0, 0.0], PI));
        assert!(!north.faces([1.0, 4.0, 0.0], PI / 2.0));
        assert!(north.faces([0.0, 0.0, 9.0], 0.1));
        let west = spot(0.0, 0.0, PI / 2.0);
        assert!(west.faces([0.0, 3.0, 0.0], 0.2) && !west.faces([3.0, 0.0, 0.0], 0.2));
    }
}
