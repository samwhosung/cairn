use std::ops::Range;

use avian3d::prelude::{Collider, RigidBody};
use bevy::prelude::*;

const CELL: f32 = 4.0;
const TROUGH_X: Range<f32> = 8.0..16.0;
const TROUGH_Z: Range<f32> = -80.0..-64.0;
const EAST_BAND_RUNS: [f32; 4] = [8.0, 8.0, 4.0, 4.0];
const EAST_BAND_SLOPES: [f32; 4] = [0.25, 0.75, 1.1, 1.6];
const EAST_TILT: f32 = -0.001;
const WEST_BUMP: f32 = 0.5;

#[derive(Default)]
struct Mesh {
    verts: Vec<Vec3>,
    tris: Vec<[u32; 3]>,
}

impl Mesh {
    fn tri_facing(&mut self, [a, b, c]: [Vec3; 3], out: Vec3) {
        let n = (b - a).cross(c - a);
        if n == Vec3::ZERO {
            return;
        }
        let base = self.verts.len() as u32;
        let (b, c) = if n.dot(out) >= 0.0 { (b, c) } else { (c, b) };
        self.verts.extend([a, b, c]);
        self.tris.push([base, base + 1, base + 2]);
    }

    fn quad_facing(&mut self, [a, b, c, d]: [Vec3; 4], out: Vec3) {
        self.tri_facing([a, b, c], out);
        self.tri_facing([a, c, d], out);
    }

    fn upright_box(&mut self, lo: Vec3, hi: Vec3) {
        let at = Vec3::new;
        let (l, h) = (lo, hi);
        self.quad_facing(
            [
                at(l.x, h.y, l.z),
                at(h.x, h.y, l.z),
                at(h.x, h.y, h.z),
                at(l.x, h.y, h.z),
            ],
            Vec3::Y,
        );
        for (x, out) in [(l.x, Vec3::NEG_X), (h.x, Vec3::X)] {
            self.quad_facing(
                [
                    at(x, l.y, l.z),
                    at(x, h.y, l.z),
                    at(x, h.y, h.z),
                    at(x, l.y, h.z),
                ],
                out,
            );
        }
        for (z, out) in [(l.z, Vec3::NEG_Z), (h.z, Vec3::Z)] {
            self.quad_facing(
                [
                    at(l.x, l.y, z),
                    at(h.x, l.y, z),
                    at(h.x, h.y, z),
                    at(l.x, h.y, z),
                ],
                out,
            );
        }
    }
}

fn ground_y(x: f32, z: f32) -> f32 {
    let east = (x - 24.0).max(0.0);
    let mut y = east * z * EAST_TILT;
    let mut left = east;
    for (run, slope) in EAST_BAND_RUNS.into_iter().zip(EAST_BAND_SLOPES) {
        y += left.min(run) * slope;
        left = (left - run).max(0.0);
    }
    if x < -20.0 && ((x / CELL) as i32 * 7 + (z / CELL) as i32 * 3).rem_euclid(5) == 0 {
        y += WEST_BUMP;
    }
    y
}

fn ground() -> Mesh {
    let mut m = Mesh::default();
    let p = |x: f32, z: f32| Vec3::new(x, ground_y(x, z), z);
    for i in 0..26 {
        for j in 0..30 {
            let (x, z) = (-48.0 + i as f32 * CELL, -96.0 + j as f32 * CELL);
            if TROUGH_X.contains(&x) && TROUGH_Z.contains(&z) {
                continue;
            }
            m.quad_facing(
                [
                    p(x, z),
                    p(x + CELL, z),
                    p(x + CELL, z + CELL),
                    p(x, z + CELL),
                ],
                Vec3::Y,
            );
        }
    }
    m
}

fn trough() -> Mesh {
    const TAN_30: f32 = 0.577_350_3;
    let axis = f32::midpoint(TROUGH_X.start, TROUGH_X.end);
    let stations = [
        (TROUGH_Z.start, 1.5),
        (-76.0, 1.5),
        (-72.0, 1.5),
        (-68.0, 0.75),
        (TROUGH_Z.end, 0.0),
    ];
    let section = |(z, depth): (f32, f32)| {
        let half_width = depth * TAN_30;
        [
            Vec3::new(TROUGH_X.start, 0.0, z),
            Vec3::new(axis - half_width, 0.0, z),
            Vec3::new(axis, -depth, z),
            Vec3::new(axis + half_width, 0.0, z),
            Vec3::new(TROUGH_X.end, 0.0, z),
        ]
    };
    let outs = [
        Vec3::Y,
        Vec3::new(1.0, 0.6, 0.0),
        Vec3::new(-1.0, 0.6, 0.0),
        Vec3::Y,
    ];
    let mut m = Mesh::default();
    for w in stations.windows(2) {
        let (a, b) = (section(w[0]), section(w[1]));
        for (k, out) in outs.into_iter().enumerate() {
            m.quad_facing([a[k], b[k], b[k + 1], a[k + 1]], out);
        }
    }
    let cap = section(stations[0]);
    m.tri_facing([cap[1], cap[2], cap[3]], Vec3::Z);
    m
}

fn nearly_coplanar_wall() -> Mesh {
    const JITTER: [f32; 4] = [0.0, 4.0e-6, -3.0e-6, 2.0e-6];
    let at = |k: usize, y: f32| Vec3::new(-20.0 + JITTER[k % 4], y, -84.0 + 4.0 * k as f32);
    let mut m = Mesh::default();
    for k in 0..10 {
        m.quad_facing(
            [at(k, -1.0), at(k, 5.0), at(k + 1, 5.0), at(k + 1, -1.0)],
            Vec3::X,
        );
    }
    m
}

fn platform_with_ramp() -> Mesh {
    let at = Vec3::new;
    let mut m = Mesh::default();
    m.upright_box(at(-6.0, 0.0, -40.0), at(6.0, 3.0, -30.0));
    m.quad_facing(
        [
            at(-4.0, 0.0, -24.0),
            at(4.0, 0.0, -24.0),
            at(4.0, 3.0, -30.0),
            at(-4.0, 3.0, -30.0),
        ],
        at(0.0, 1.0, 0.5),
    );
    for (x, out) in [(-4.0, Vec3::NEG_X), (4.0, Vec3::X)] {
        m.tri_facing(
            [at(x, 0.0, -24.0), at(x, 3.0, -30.0), at(x, 0.0, -30.0)],
            out,
        );
    }
    m
}

fn kerb() -> Mesh {
    let (rise, bevel_run) = (0.28, 0.156);
    let (x0, x1, z0, z1) = (-18.0, -2.0, -51.0, -48.0);
    let profile = [
        (z1, 0.0),
        (z1 - bevel_run, rise),
        (z0 + bevel_run, rise),
        (z0, 0.0),
    ];
    let outs = [Vec3::new(0.0, 0.5, 1.0), Vec3::Y, Vec3::new(0.0, 0.5, -1.0)];
    let mut m = Mesh::default();
    for (w, out) in profile.windows(2).zip(outs) {
        let ((za, ya), (zb, yb)) = (w[0], w[1]);
        m.quad_facing(
            [
                Vec3::new(x0, ya, za),
                Vec3::new(x1, ya, za),
                Vec3::new(x1, yb, zb),
                Vec3::new(x0, yb, zb),
            ],
            out,
        );
    }
    for (x, out) in [(x0, Vec3::NEG_X), (x1, Vec3::X)] {
        m.quad_facing(profile.map(|(z, y)| Vec3::new(x, y, z)), out);
    }
    m
}

pub fn spawn(world: &mut World) {
    let mut block = Mesh::default();
    block.upright_box(Vec3::new(17.0, 0.0, -58.0), Vec3::new(23.0, 1.4, -54.0));
    let meshes = [
        ground(),
        trough(),
        nearly_coplanar_wall(),
        platform_with_ramp(),
        kerb(),
        block,
    ];
    for Mesh { verts, tris } in meshes {
        world.spawn((
            RigidBody::Static,
            Collider::trimesh(verts, tris),
            Transform::default(),
        ));
    }
    world.spawn((
        RigidBody::Static,
        Collider::cuboid(1.0, 4.0, 1.0),
        Transform::from_xyz(-0.3, 2.0, -12.0),
    ));
}
