//! Ground decals: the receiving triangles inside a box, clipped to it and textured from above, so
//! a decal lies on the ground it marks and drapes down the face of a step.

use avian3d::parry::bounding_volume::{Aabb as ParryAabb, BoundingVolume};
use avian3d::prelude::Collider;
use bevy::prelude::*;

use crate::collision::GroundDecalSurface;
use crate::effects::EffectVertex;

/// A decal's box about `center`: a rectangle in the frame `turn` takes world offsets into, and a
/// slab from `min_y` to `max_y`.
pub(crate) struct DecalFrame {
    pub center: Vec3,
    pub turn: Rot2,
    pub min_x: f32,
    pub max_x: f32,
    pub min_z: f32,
    pub max_z: f32,
    pub min_y: f32,
    pub max_y: f32,
}

impl DecalFrame {
    fn in_frame(&self, p: Vec3) -> Vec2 {
        self.turn * Vec2::new(p.x - self.center.x, p.z - self.center.z)
    }

    pub fn rect_uv(&self, x: f32, z: f32) -> [f32; 2] {
        [
            (x - self.min_x) / (self.max_x - self.min_x),
            (z - self.min_z) / (self.max_z - self.min_z),
        ]
    }

    fn gather_aabb(&self) -> ParryAabb {
        let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for corner in [
            Vec2::new(self.min_x, self.min_z),
            Vec2::new(self.min_x, self.max_z),
            Vec2::new(self.max_x, self.min_z),
            Vec2::new(self.max_x, self.max_z),
        ] {
            let d = self.turn.inverse() * corner;
            lo = lo.min(d);
            hi = hi.max(d);
        }
        ParryAabb::new(
            self.center + Vec3::new(lo.x, self.min_y, lo.y),
            self.center + Vec3::new(hi.x, self.max_y, hi.y),
        )
    }
}

const PLANES: usize = 6;
const MAX_CORNERS: usize = 3 + PLANES;

/// Clips a triangle to the frame's box, keeping it on its plane; `None` when nothing is left, or
/// when rounding would give it more corners than a convex clip can have.
fn clip_to_frame(tri: [Vec3; 3], frame: &DecalFrame) -> Option<([Vec3; MAX_CORNERS], usize)> {
    let planes: [&dyn Fn(Vec3) -> f32; PLANES] = [
        &|p: Vec3| frame.max_x - frame.in_frame(p).x,
        &|p: Vec3| frame.in_frame(p).x - frame.min_x,
        &|p: Vec3| frame.max_z - frame.in_frame(p).y,
        &|p: Vec3| frame.in_frame(p).y - frame.min_z,
        &|p: Vec3| (frame.center.y + frame.max_y) - p.y,
        &|p: Vec3| p.y - (frame.center.y + frame.min_y),
    ];
    let mut poly = [Vec3::ZERO; MAX_CORNERS];
    let mut next = [Vec3::ZERO; MAX_CORNERS];
    poly[..3].copy_from_slice(&tri);
    let mut len = 3;
    for dist in planes {
        let mut out = 0;
        for i in 0..len {
            let (a, b) = (poly[i], poly[(i + 1) % len]);
            let (da, db) = (dist(a), dist(b));
            let crosses = (da >= 0.0) != (db >= 0.0);
            if out + usize::from(da >= 0.0) + usize::from(crosses) > MAX_CORNERS {
                return None;
            }
            if da >= 0.0 {
                next[out] = a;
                out += 1;
            }
            if crosses {
                next[out] = a + (b - a) * (da / (da - db));
                out += 1;
            }
        }
        std::mem::swap(&mut poly, &mut next);
        len = out;
        if len < 3 {
            return None;
        }
    }
    Some((poly, len))
}

/// Projects decals onto the colliders that take them.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct WorldDecal<'w, 's> {
    surfaces: Query<'w, 's, &'static Collider, With<GroundDecalSurface>>,
}

impl WorldDecal<'_, '_> {
    /// Appends the receiving triangles inside `frame`'s box to `out` as world-space triangles,
    /// white, with the alpha `alpha` gives a corner's `(x', y − center.y, z')` and the texture
    /// coordinate `uv` gives its `(x', z')`. `false` when nothing in the box receives it.
    pub fn project(
        &self,
        out: &mut Vec<EffectVertex>,
        frame: &DecalFrame,
        alpha: impl Fn(Vec3) -> f32,
        uv: impl Fn(f32, f32) -> [f32; 2],
    ) -> bool {
        if frame.max_x - frame.min_x <= 0.0 || frame.max_z - frame.min_z <= 0.0 {
            return false;
        }
        let gather = frame.gather_aabb();
        let start = out.len();
        for collider in &self.surfaces {
            let Some(trimesh) = collider.shape().as_trimesh() else {
                continue;
            };
            if !trimesh.local_aabb().intersects(&gather) {
                continue;
            }
            for i in trimesh.bvh().intersect_aabb(&gather) {
                let tri = trimesh.triangle(i);
                let Some((poly, n)) = clip_to_frame([tri.a, tri.b, tri.c], frame) else {
                    continue;
                };
                let vert = |p: Vec3| {
                    let at = frame.in_frame(p);
                    EffectVertex {
                        pos: p.to_array(),
                        uv: uv(at.x, at.y),
                        color: [
                            1.0,
                            1.0,
                            1.0,
                            alpha(Vec3::new(at.x, p.y - frame.center.y, at.y)),
                        ],
                    }
                };
                for k in 1..n - 1 {
                    out.extend([vert(poly[0]), vert(poly[k]), vert(poly[k + 1])]);
                }
            }
        }
        out.len() > start
    }

    /// How many receivers are in, which changes as the ground streams.
    pub fn receiver_count(&self) -> usize {
        self.surfaces.iter().count()
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    fn frame() -> DecalFrame {
        DecalFrame {
            center: Vec3::ZERO,
            turn: Rot2::IDENTITY,
            min_x: -1.0,
            max_x: 1.0,
            min_z: -1.0,
            max_z: 1.0,
            min_y: -1.0,
            max_y: 1.0,
        }
    }

    #[test]
    fn a_triangle_inside_the_box_passes_through_unchanged() {
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.5, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.5),
        ];
        let (poly, n) = clip_to_frame(tri, &frame()).expect("inside");
        assert_eq!(n, 3);
        assert_eq!(&poly[..3], &tri);
    }

    #[test]
    fn a_half_clipped_triangle_keeps_each_corner_then_its_edge_crossing() {
        let tri = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 0.5),
        ];
        let (poly, n) = clip_to_frame(tri, &frame()).expect("half inside");
        assert_eq!(n, 4);
        assert_eq!(
            &poly[..4],
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.25),
                Vec3::new(0.0, 0.0, 0.5),
            ]
        );
    }

    #[test]
    fn a_triangle_outside_the_box_clips_to_nothing() {
        let tri = [
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 1.0),
        ];
        assert!(clip_to_frame(tri, &frame()).is_none());
    }

    fn ground(y: impl Fn(f32, f32) -> f32) -> Collider {
        let (mut verts, mut tris) = (Vec::new(), Vec::new());
        for i in -5..5 {
            for j in -5..5 {
                let b = verts.len() as u32;
                for (x, z) in [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)] {
                    let (x, z) = (x as f32, z as f32);
                    verts.push(Vec3::new(x, y(x, z), z));
                }
                tris.extend([[b, b + 2, b + 1], [b, b + 3, b + 2]]);
            }
        }
        Collider::trimesh(verts, tris)
    }

    fn project(receivers: Vec<(Collider, bool)>, frame: DecalFrame) -> Option<Vec<EffectVertex>> {
        let mut world = World::new();
        for (collider, marked) in receivers {
            let mut e = world.spawn(collider);
            if marked {
                e.insert(GroundDecalSurface);
            }
        }
        world
            .run_system_once(move |decals: WorldDecal<'_, '_>| {
                let mut out = Vec::new();
                decals
                    .project(&mut out, &frame, |_| 1.0, |x, z| frame.rect_uv(x, z))
                    .then_some(out)
            })
            .expect("the system runs")
    }

    fn footprint(frame: DecalFrame) -> DecalFrame {
        DecalFrame {
            center: Vec3::new(0.25, 0.0, -0.5),
            min_x: -0.5,
            max_x: 0.5,
            min_z: -0.75,
            max_z: 0.75,
            ..frame
        }
    }

    /// The area the triangles cover seen from above, and their true area.
    fn areas(verts: &[EffectVertex]) -> (f32, f32) {
        verts.chunks(3).fold((0.0, 0.0), |(top, full), t| {
            let [a, b, c] = [t[0].pos, t[1].pos, t[2].pos].map(Vec3::from);
            let n = (b - a).cross(c - a);
            (top + n.y.abs() / 2.0, full + n.length() / 2.0)
        })
    }

    #[test]
    fn a_flat_ground_takes_the_whole_rectangle_and_the_texture_square() {
        let verts = project(vec![(ground(|_, _| 0.0), true)], footprint(frame()))
            .expect("the ground receives it");
        let (top, full) = areas(&verts);
        assert!((top - 1.5).abs() < 1e-5 && (full - 1.5).abs() < 1e-5);
        for v in &verts {
            assert!(v.pos[1].abs() < 1e-6);
            assert!(v.uv.iter().all(|t| (-1e-5..=1.0 + 1e-5).contains(t)));
        }
        let corner = |u: f32, v: f32| {
            verts
                .iter()
                .any(|x| (x.uv[0] - u).abs() < 1e-5 && (x.uv[1] - v).abs() < 1e-5)
        };
        assert!(corner(0.0, 0.0) && corner(1.0, 1.0));
    }

    #[test]
    fn a_slope_takes_the_rectangle_from_above_and_stays_on_its_plane() {
        let verts = project(vec![(ground(|x, _| 0.5 * x), true)], footprint(frame()))
            .expect("the slope receives it");
        let (top, full) = areas(&verts);
        assert!((top - 1.5).abs() < 1e-4);
        assert!((full - 1.5 * 1.25f32.sqrt()).abs() < 1e-4, "{full}");
        for v in &verts {
            assert!((v.pos[1] - 0.5 * v.pos[0]).abs() < 1e-5);
        }
    }

    #[test]
    fn a_step_drapes_the_decal_down_its_face() {
        let lower = ground(|_, _| 0.0);
        let upper = Collider::trimesh(
            vec![
                Vec3::new(0.0, 0.5, -5.0),
                Vec3::new(5.0, 0.5, -5.0),
                Vec3::new(5.0, 0.5, 5.0),
                Vec3::new(0.0, 0.5, 5.0),
                Vec3::new(0.0, 0.0, -5.0),
                Vec3::new(0.0, 0.0, 5.0),
            ],
            vec![[0, 2, 1], [0, 3, 2], [4, 0, 5], [5, 0, 3]],
        );
        let verts = project(vec![(lower, true), (upper, true)], footprint(frame()))
            .expect("the step receives it");
        let face: Vec<&EffectVertex> = verts
            .chunks(3)
            .filter(|t| t.iter().all(|v| v.pos[0].abs() < 1e-6))
            .flatten()
            .collect();
        assert!(!face.is_empty(), "the riser takes the decal");
        let u = face[0].uv[0];
        assert!(
            face.iter().all(|v| (v.uv[0] - u).abs() < 1e-6),
            "a riser smears one column of the texture"
        );
        assert!(face.iter().any(|v| v.pos[1] > 0.49) && face.iter().any(|v| v.pos[1] < 0.01));
    }

    #[test]
    fn nothing_under_the_box_is_no_decal() {
        let far_below = ground(|_, _| -3.0);
        assert!(project(vec![(far_below, true)], frame()).is_none());
        assert!(
            project(vec![(ground(|_, _| 0.0), false)], frame()).is_none(),
            "a collider not marked as ground takes nothing"
        );
    }

    #[test]
    fn a_turned_box_gathers_what_its_corners_reach() {
        let turned = DecalFrame {
            turn: Rot2::radians(std::f32::consts::FRAC_PI_4),
            ..footprint(frame())
        };
        let verts = project(vec![(ground(|_, _| 0.0), true)], turned).expect("received");
        let (top, _) = areas(&verts);
        assert!((top - 1.5).abs() < 1e-4, "{top}");
    }
}
