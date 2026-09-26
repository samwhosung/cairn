//! A chunk's normals from its heights, baked as the shipped tiles were: each vertex's normal is
//! the sum of the unit normals of the triangles that touch it, renormalised, scaled by 127 and
//! truncated toward zero, all in f32. A cell is a fan of four triangles from its centre, so an
//! outer vertex touches eight and an inner vertex four. The bytes are `[x, y, z]` in world axes.
//!
//! A vertex stored by several chunks reads one copy, the south- and east-most chunk's of the tile
//! it lies in, so every copy bakes to the same normal.

const CELL: f32 = crate::frame::CELL as f32;

/// One tile's heights as its chunks store them: each chunk's base and its 145 heights over it.
#[derive(Clone, Debug, PartialEq)]
pub struct TileHeights {
    pub base: Vec<f32>,
    pub mcvt: Vec<[f32; 145]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub row: usize,
    pub col: usize,
    pub inner: bool,
}

impl Vertex {
    /// The vertex a chunk's `MCVT` holds at `i`: rows of 9 outer, then 8 inner.
    pub fn at(i: usize) -> Vertex {
        let (row, col) = (i / 17, i % 17);
        Vertex {
            row,
            col: if col < 9 { col } else { col - 9 },
            inner: col >= 9,
        }
    }
}

impl TileHeights {
    fn outer_at(&self, r: isize, c: isize) -> f32 {
        let (or, oc) = ((r / 8).min(15), (c / 8).min(15));
        let owner = (or * 16 + oc) as usize;
        self.base[owner] + self.mcvt[owner][((r - or * 8) * 17 + c - oc * 8) as usize]
    }

    fn inner_at(&self, r: isize, c: isize) -> f32 {
        let owner = ((r / 8) * 16 + c / 8) as usize;
        self.base[owner] + self.mcvt[owner][((r % 8) * 17 + 9 + c % 8) as usize]
    }
}

#[derive(Clone, Copy)]
pub struct Field<'a> {
    pub tile: &'a TileHeights,
    around: [[Option<&'a TileHeights>; 3]; 3],
}

impl<'a> Field<'a> {
    pub fn alone(tile: &'a TileHeights) -> Field<'a> {
        Field {
            tile,
            around: [[None; 3]; 3],
        }
    }

    /// Give the tile `dx` east and `dy` south of this one, each -1 to 1.
    pub fn set_neighbour(&mut self, dx: isize, dy: isize, h: Option<&'a TileHeights>) {
        self.around[(dy + 1) as usize][(dx + 1) as usize] = h;
    }

    fn neighbour(&self, dx: isize, dy: isize) -> Option<&'a TileHeights> {
        if (dx, dy) == (0, 0) {
            Some(self.tile)
        } else {
            *self.around.get((dy + 1) as usize)?.get((dx + 1) as usize)?
        }
    }

    /// Outer lattice point `(rr, cc)`, the tile spanning 0..=128 and its neighbours beyond.
    fn outer(&self, rr: isize, cc: isize) -> Option<f32> {
        let step = |v: isize| isize::from(v > 128) - isize::from(v < 0);
        let (tr, tc) = (step(rr), step(cc));
        let h = self.neighbour(tc, tr)?;
        Some(h.outer_at(rr - tr * 128, cc - tc * 128))
    }

    /// Inner lattice point `(rr, cc)`, the tile spanning 0..128.
    fn inner(&self, rr: isize, cc: isize) -> Option<f32> {
        let (tr, tc) = (rr.div_euclid(128), cc.div_euclid(128));
        let h = self.neighbour(tc, tr)?;
        Some(h.inner_at(rr - tr * 128, cc - tc * 128))
    }
}

fn unit(a: [f32; 3]) -> [f32; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

/// The unit normal of triangle `(me, x, y)`, turned to face up.
fn face(me: [f32; 3], x: [f32; 3], y: [f32; 3]) -> [f32; 3] {
    let a = [x[0] - me[0], x[1] - me[1], x[2] - me[2]];
    let b = [y[0] - me[0], y[1] - me[1], y[2] - me[2]];
    let mut n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    if n[2] < 0.0 {
        n = [-n[0], -n[1], -n[2]];
    }
    unit(n)
}

fn add(s: &mut [f32; 3], t: [f32; 3]) {
    for j in 0..3 {
        s[j] += t[j];
    }
}

pub fn bake(field: &Field<'_>, chunk: usize, vertex: usize) -> [u8; 3] {
    let v = Vertex::at(vertex);
    let (er, ec) = ((chunk / 16) as isize, (chunk % 16) as isize);
    let (rr, cc) = (er * 8 + v.row as isize, ec * 8 + v.col as isize);
    let u = CELL;
    let mut s = [0f32; 3];
    if v.inner {
        let corners = (|| {
            Some((
                [0.5 * u, 0.5 * u, field.inner(rr, cc)?],
                [
                    [0.0, 0.0, field.outer(rr, cc)?],
                    [u, 0.0, field.outer(rr + 1, cc)?],
                    [u, u, field.outer(rr + 1, cc + 1)?],
                    [0.0, u, field.outer(rr, cc + 1)?],
                ],
            ))
        })();
        if let Some((me, c)) = corners {
            for k in 0..4 {
                add(&mut s, face(me, c[k], c[(k + 1) % 4]));
            }
        }
    } else if let Some(h) = field.outer(rr, cc) {
        let me = [0.0, 0.0, h];
        for (dr, dc) in [(-1isize, -1isize), (-1, 0), (0, -1), (0, 0)] {
            let ea: isize = if dr == -1 { -1 } else { 1 };
            let eb: isize = if dc == -1 { -1 } else { 1 };
            let cell = (|| {
                Some((
                    [
                        (dr as f32 + 0.5) * u,
                        (dc as f32 + 0.5) * u,
                        field.inner(rr + dr, cc + dc)?,
                    ],
                    [ea as f32 * u, 0.0, field.outer(rr + ea, cc)?],
                    [0.0, eb as f32 * u, field.outer(rr, cc + eb)?],
                ))
            })();
            if let Some((centre, a, b)) = cell {
                add(&mut s, face(me, a, centre));
                add(&mut s, face(me, centre, b));
            }
        }
    }
    let n = unit(s);
    [
        (-n[0] * 127.0) as i32 as i8 as u8,
        (-n[1] * 127.0) as i32 as i8 as u8,
        (n[2] * 127.0) as i32 as i8 as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(z: f32) -> TileHeights {
        TileHeights {
            base: vec![z; 256],
            mcvt: vec![[0.0; 145]; 256],
        }
    }

    #[test]
    fn flat_ground_points_straight_up() {
        let h = flat(12.5);
        for e in [0, 17, 255] {
            for i in [0, 8, 9, 60, 144] {
                assert_eq!(bake(&Field::alone(&h), e, i), [0, 0, 127]);
            }
        }
    }

    #[test]
    fn a_slope_tilts_the_normal_downhill() {
        let mut h = flat(0.0);
        for e in 0..256 {
            for i in 0..145 {
                let v = Vertex::at(i);
                let row = (e / 16) as f32 * 8.0 + v.row as f32 + if v.inner { 0.5 } else { 0.0 };
                h.mcvt[e][i] = row * CELL;
            }
        }
        assert_eq!(bake(&Field::alone(&h), 17, 20), [89, 0, 89]);
        assert_eq!(bake(&Field::alone(&h), 17, 26), [89, 0, 89]);
    }

    #[test]
    fn a_neighbour_tile_shapes_the_border() {
        let h = flat(0.0);
        let mut north = flat(0.0);
        for e in 240..256 {
            for c in 0..17 {
                north.mcvt[e][7 * 17 + c] = 4.0;
            }
        }
        let mut f = Field::alone(&h);
        f.set_neighbour(0, -1, Some(&north));
        assert!(
            (bake(&f, 3, 4)[0] as i8) < 0,
            "high ground north tilts it south"
        );
        assert_eq!(bake(&Field::alone(&h), 3, 4), [0, 0, 127]);
    }
}
