use crate::world::Body;

/// A uniform grid over the living entities' bounding box, rebuilt each tick by counting sort:
/// its cost follows the entities and the box they span, not the map.
pub struct Grid {
    cell: f32,
    origin: [f32; 2],
    w: usize,
    h: usize,
    /// Where each cell's slots start in `items`; one more entry than there are cells.
    starts: Vec<u32>,
    items: Vec<u32>,
    cell_of: Vec<u32>,
}

impl Grid {
    pub fn new(cell: f32) -> Self {
        Self {
            cell,
            origin: [0.0; 2],
            w: 0,
            h: 0,
            starts: vec![0],
            items: Vec::new(),
            cell_of: Vec::new(),
        }
    }

    pub fn rebuild(&mut self, bodies: &[Body]) {
        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for b in bodies.iter().filter(|b| b.alive) {
            for a in 0..2 {
                lo[a] = lo[a].min(b.movement.pos[a]);
                hi[a] = hi[a].max(b.movement.pos[a]);
            }
        }
        if lo[0] > hi[0] {
            (lo, hi) = ([0.0; 2], [0.0; 2]);
        }
        self.origin = lo;
        self.w = ((hi[0] - lo[0]) / self.cell) as usize + 1;
        self.h = ((hi[1] - lo[1]) / self.cell) as usize + 1;
        self.starts.clear();
        self.starts.resize(self.w * self.h + 1, 0);
        self.cell_of.clear();
        self.items.clear();
        for b in bodies {
            let c = if b.alive {
                self.cell_index(b.movement.pos)
            } else {
                u32::MAX
            };
            self.cell_of.push(c);
            if c != u32::MAX {
                self.starts[c as usize + 1] += 1;
            }
        }
        for i in 0..self.w * self.h {
            self.starts[i + 1] += self.starts[i];
        }
        self.items.resize(self.starts[self.w * self.h] as usize, 0);
        let mut fill = self.starts.clone();
        for (slot, &c) in self.cell_of.iter().enumerate() {
            if c != u32::MAX {
                self.items[fill[c as usize] as usize] = slot as u32;
                fill[c as usize] += 1;
            }
        }
    }

    fn cell_xy(&self, pos: [f32; 3]) -> (usize, usize) {
        let x = ((pos[0] - self.origin[0]) / self.cell).max(0.0) as usize;
        let y = ((pos[1] - self.origin[1]) / self.cell).max(0.0) as usize;
        (x.min(self.w - 1), y.min(self.h - 1))
    }

    fn cell_index(&self, pos: [f32; 3]) -> u32 {
        let (x, y) = self.cell_xy(pos);
        (y * self.w + x) as u32
    }

    /// Pushes every living slot whose ground position lies within `r` of `at`'s.
    pub fn query(&self, bodies: &[Body], at: [f32; 3], r: f32, out: &mut Vec<u32>) {
        let (x0, y0) = self.cell_xy([at[0] - r, at[1] - r, 0.0]);
        let (x1, y1) = self.cell_xy([at[0] + r, at[1] + r, 0.0]);
        let r2 = r * r;
        for y in y0..=y1 {
            let row = y * self.w;
            let cells = self.starts[row + x0] as usize..self.starts[row + x1 + 1] as usize;
            for &slot in &self.items[cells] {
                let p = bodies[slot as usize].movement.pos;
                let (dx, dy) = (p[0] - at[0], p[1] - at[1]);
                if dx * dx + dy * dy <= r2 {
                    out.push(slot);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::Movement;

    fn at(x: f32, y: f32, alive: bool) -> Body {
        Body {
            movement: Movement {
                pos: [x, y, 0.0],
                ..Movement::default()
            },
            alive,
            ..Body::default()
        }
    }

    #[test]
    fn a_query_finds_exactly_the_living_within_range() {
        let bodies: Vec<Body> = (0..400)
            .map(|i| at((i % 20) as f32 * 13.0, (i / 20) as f32 * 11.0, i % 7 != 0))
            .collect();
        let mut grid = Grid::new(50.0);
        grid.rebuild(&bodies);
        for (centre, r) in [([0.0, 0.0, 0.0], 30.0), ([120.0, 100.0, 9.0], 101.0)] {
            let mut got = Vec::new();
            grid.query(&bodies, centre, r, &mut got);
            got.sort_unstable();
            let want: Vec<u32> = (0..bodies.len() as u32)
                .filter(|&i| {
                    let p = bodies[i as usize].movement.pos;
                    let d2 = (p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2);
                    bodies[i as usize].alive && d2 <= r * r
                })
                .collect();
            assert_eq!(got, want);
        }
    }

    #[test]
    fn an_empty_world_answers_nothing() {
        let mut grid = Grid::new(50.0);
        grid.rebuild(&[at(5.0, 5.0, false)]);
        let mut got = Vec::new();
        grid.query(&[at(5.0, 5.0, false)], [5.0, 5.0, 0.0], 100.0, &mut got);
        assert!(got.is_empty());
    }
}
