//! WoW's axes and Bevy's. WoW is right-handed with +x north, +y west and +z up; Bevy is
//! right-handed with +y up and -z forward. The map between them is the rotation
//! `bevy = (-y, z, -x)`: it never mirrors, and a unit stays a yard.

use bevy::math::{Mat3, Quat, Vec3};
use bevy::transform::components::Transform;

/// A WoW world or model position `[x, y, z]` in Bevy's axes.
pub fn wow_to_bevy(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[1], p[2], -p[0])
}

/// The inverse of [`wow_to_bevy`].
pub fn bevy_to_wow(b: Vec3) -> [f32; 3] {
    [-b.z, -b.x, b.y]
}

fn wow_to_bevy_quat() -> Quat {
    Quat::from_mat3(&Mat3::from_cols(
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(-1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ))
}

/// The heading an MDDF or MODF entry gives a model whose front faces north.
pub const NORTH_HEADING_DEG: f32 = 180.0;

/// An MDDF or MODF entry's heading for a model facing `facing_deg`, from north toward west.
pub fn heading_of(facing_deg: f32) -> f32 {
    (facing_deg + NORTH_HEADING_DEG).rem_euclid(360.0)
}

/// The inverse of [`heading_of`].
pub fn facing_of(heading_deg: f32) -> f32 {
    (heading_deg - NORTH_HEADING_DEG).rem_euclid(360.0)
}

/// The rotation of a model placed by an ADT's MDDF or MODF entry, from its angles in degrees, for
/// meshes already in Bevy's axes: `ry` turns it about the up axis, `rx` and `rz` pitch and roll it.
pub fn placement_rotation(rotation_deg: [f32; 3]) -> Quat {
    use std::f32::consts::FRAC_PI_2;
    let (rx, ry, rz) = (
        rotation_deg[0].to_radians(),
        rotation_deg[1].to_radians(),
        rotation_deg[2].to_radians(),
    );
    let in_wow = Quat::from_rotation_x(FRAC_PI_2)
        * Quat::from_rotation_y(ry - NORTH_HEADING_DEG.to_radians())
        * Quat::from_rotation_z(-rx)
        * Quat::from_rotation_x(rz - FRAC_PI_2);
    let to_bevy = wow_to_bevy_quat();
    to_bevy * in_wow * to_bevy.inverse()
}

/// Where a WMO's doodad (an MODD entry) sits in its WMO's space, in Bevy's axes: compose it onto
/// the WMO's own transform.
pub fn wmo_doodad_local(position: [f32; 3], orientation: [f32; 4], scale: f32) -> Transform {
    Transform {
        translation: wow_to_bevy(position),
        rotation: wow_rotation_to_bevy(orientation),
        scale: Vec3::splat(scale),
    }
}

/// A rotation stored in WoW's axes as `[x, y, z, w]`, in Bevy's; normalized, as some stored
/// quaternions are not.
pub fn wow_rotation_to_bevy(q: [f32; 4]) -> Quat {
    let q_wow = Quat::from_xyzw(q[0], q[1], q[2], q[3]);
    let to_bevy = wow_to_bevy_quat();
    (to_bevy * q_wow * to_bevy.inverse()).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-5
    }

    fn same_rotation(a: Quat, b: Quat) -> bool {
        a.dot(b).abs() > 0.9999
    }

    #[test]
    fn wow_bevy_round_trips() {
        for p in [
            [1.0, 2.0, 3.0],
            [-8949.95, -132.49, 83.53],
            [0.0, 0.0, 0.0],
            [100.0, -50.0, 7.0],
        ] {
            let back = bevy_to_wow(wow_to_bevy(p));
            assert!(
                (0..3).all(|i| (back[i] - p[i]).abs() < 1e-3),
                "round-trip {p:?} → {back:?}"
            );
        }
    }

    #[test]
    fn a_heading_about_wows_up_is_one_about_bevys() {
        for theta in [0.0f32, 0.7, 2.4, -1.1, 5.9] {
            let (s, c) = theta.sin_cos();
            for v in [[3.0f32, -2.0, 1.5], [10.0, 0.0, -4.0]] {
                let rotated_wow = [c * v[0] - s * v[1], s * v[0] + c * v[1], v[2]];
                let a = wow_to_bevy(rotated_wow);
                let b = Quat::from_rotation_y(theta) * wow_to_bevy(v);
                assert!(close(a, b), "θ={theta}: {a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn wow_bevy_basis_is_golden() {
        assert!(close(
            wow_to_bevy([1.0, 0.0, 0.0]),
            Vec3::new(0.0, 0.0, -1.0)
        ));
        assert!(close(
            wow_to_bevy([0.0, 1.0, 0.0]),
            Vec3::new(-1.0, 0.0, 0.0)
        ));
        assert!(close(
            wow_to_bevy([0.0, 0.0, 1.0]),
            Vec3::new(0.0, 1.0, 0.0)
        ));
    }

    #[test]
    fn placement_quat_matches_the_position_transform() {
        for w in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [3.0, -2.0, 5.0],
        ] {
            let via_quat = wow_to_bevy_quat() * Vec3::new(w[0], w[1], w[2]);
            assert!(close(via_quat, wow_to_bevy(w)), "quat vs fn for {w:?}");
        }
    }

    #[test]
    fn transform_is_a_proper_rotation() {
        let m = Mat3::from_quat(wow_to_bevy_quat());
        assert!(
            (m.determinant() - 1.0).abs() < 1e-5,
            "det = {}",
            m.determinant()
        );
    }

    #[test]
    fn a_model_turned_to_a_facing_faces_it() {
        let north_bits = NORTH_HEADING_DEG.to_radians().to_bits();
        assert_eq!(
            north_bits,
            std::f32::consts::PI.to_bits(),
            "π to the last bit"
        );
        for facing in [0.0_f32, 30.0, 90.0, 200.0, 359.5] {
            let q = placement_rotation([0.0, heading_of(facing), 0.0]);
            let front = Vec3::from(bevy_to_wow(q * wow_to_bevy([1.0, 0.0, 0.0])));
            let r = facing.to_radians();
            let want = Vec3::new(r.cos(), r.sin(), 0.0);
            assert!(close(front, want), "facing {facing}: {front}");
            assert!((facing_of(heading_of(facing)) - facing).abs() < 1e-4);
        }
    }

    #[test]
    fn heading_only_placement_spins_about_vertical() {
        for deg in [0.0, 30.0, 90.0, 250.0] {
            let q = placement_rotation([0.0, deg, 0.0]);
            assert!(
                close(q * Vec3::Y, Vec3::Y),
                "heading {deg}° should fix +Y, got {:?}",
                q * Vec3::Y
            );
        }
    }

    #[test]
    fn wmo_doodad_identity_is_translate_scale() {
        let t = wmo_doodad_local([3.0, -2.0, 5.0], [0.0, 0.0, 0.0, 1.0], 2.5);
        assert!(close(t.translation, wow_to_bevy([3.0, -2.0, 5.0])));
        assert!(
            same_rotation(t.rotation, Quat::IDENTITY),
            "rot = {:?}",
            t.rotation
        );
        assert!((t.scale - Vec3::splat(2.5)).length() < 1e-5);
    }

    #[test]
    fn wmo_doodad_at_origin_lands_on_instance() {
        let wmo_world = Transform {
            translation: Vec3::new(10.0, 20.0, -30.0),
            rotation: placement_rotation([0.0, 137.0, 0.0]),
            scale: Vec3::ONE,
        };
        let local = wmo_doodad_local([0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0], 1.0);
        let world = wmo_world.mul_transform(local);
        assert!(close(world.translation, wmo_world.translation));
    }

    #[test]
    fn wmo_doodad_heading_spins_about_vertical() {
        for deg in [0.0_f32, 35.0, 90.0, 215.0] {
            let q = Quat::from_rotation_z(deg.to_radians());
            let t = wmo_doodad_local([0.0, 0.0, 0.0], q.to_array(), 1.0);
            assert!(
                close(t.rotation * Vec3::Y, Vec3::Y),
                "heading {deg}° should fix +Y, got {:?}",
                t.rotation * Vec3::Y
            );
        }
    }

    #[test]
    fn placement_rotations_are_unit_quaternions() {
        for r in [[0.0, 0.0, 0.0], [12.0, 34.0, 56.0], [-90.0, 180.0, 45.0]] {
            let q = placement_rotation(r);
            assert!((q.length() - 1.0).abs() < 1e-4, "non-unit quat for {r:?}");
        }
    }

    #[test]
    fn placement_rotation_goldens() {
        use std::f32::consts::FRAC_1_SQRT_2;
        let cases = [
            (
                [86.0_f32, 252.0, 93.5],
                Quat::from_xyzw(-0.691_160, -0.107_332, -0.156_292, 0.697_388),
            ),
            (
                [0.0, 0.0, 90.0],
                Quat::from_xyzw(FRAC_1_SQRT_2, -FRAC_1_SQRT_2, 0.0, 0.0),
            ),
            (
                [30.0, 45.0, 60.0],
                Quat::from_xyzw(0.360_423, -0.822_363, -0.391_904, 0.200_562),
            ),
        ];
        for (deg, expected) in cases {
            let q = placement_rotation(deg);
            assert!(
                same_rotation(q, expected),
                "placement_rotation({deg:?}) = {q:?}, expected ~{expected:?}"
            );
        }
    }
}
