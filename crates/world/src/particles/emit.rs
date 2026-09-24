use bevy::prelude::*;
use model::{ParamsNow, ParticleEmitterDef, ParticleShape};

pub(crate) fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

pub(crate) fn rand01(state: &mut u32) -> f32 {
    (xorshift32(state) >> 8) as f32 / (1u32 << 24) as f32
}

pub(crate) fn rand_signed(state: &mut u32) -> f32 {
    rand01(state) * 2.0 - 1.0
}

/// The client turns every emitter's frame a quarter turn about +Z.
pub(super) fn emitter_frame_turn(v: Vec3) -> Vec3 {
    Vec3::new(-v.y, v.x, v.z)
}

pub(super) fn emitter_frame_turn_rotation() -> Quat {
    Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)
}

pub(crate) fn emit_local(def: &ParticleEmitterDef, now: &ParamsNow, rng: &mut u32) -> (Vec3, Vec3) {
    let origin = Vec3::from(def.position);
    if let (ParticleShape::Spline, Some(spline)) = (def.shape, &def.spline) {
        let (t0, t1) = (
            now.area_length.clamp(0.0, 1.0),
            now.area_width.clamp(0.0, 1.0),
        );
        let t = t0 + rand01(rng) * (t1 - t0);
        let mut pos = origin + Vec3::from(spline.eval(t));
        let dir = if now.z_source != 0.0 {
            (pos - origin - Vec3::new(0.0, 0.0, now.z_source)).normalize_or(Vec3::Z)
        } else if now.vertical_range != 0.0 {
            let tangent = Vec3::from(spline.tangent(t)).normalize_or(Vec3::Z);
            let (s, c) = (rand_signed(rng) * now.vertical_range).sin_cos();
            let dir = Vec3::Z * c + tangent.cross(Vec3::Z) * s + tangent * (tangent.z * (1.0 - c));
            if now.horizontal_range != 0.0 {
                pos += rand01(rng) * now.horizontal_range * dir;
            }
            dir
        } else {
            Vec3::ZERO
        };
        return (
            origin + emitter_frame_turn(pos - origin),
            emitter_frame_turn(dir),
        );
    }
    let (local, shell) = if def.shape == ParticleShape::Sphere {
        let r = now.area_length + rand01(rng) * (now.area_width - now.area_length).max(0.0);
        let lat = rand_signed(rng) * now.vertical_range;
        let lon = rand_signed(rng) * now.horizontal_range;
        let (slat, clat) = lat.sin_cos();
        let (slon, clon) = lon.sin_cos();
        let shell = Vec3::new(clat * clon, clat * slon, slat);
        (r * shell, Some(shell))
    } else {
        (
            Vec3::new(
                rand_signed(rng) * 0.5 * now.area_length,
                rand_signed(rng) * 0.5 * now.area_width,
                0.0,
            ),
            None,
        )
    };
    let dir = if now.z_source != 0.0 {
        (local - Vec3::new(0.0, 0.0, now.z_source)).normalize_or(Vec3::Z)
    } else if let Some(shell) = shell {
        if def.sphere_up() { Vec3::Z } else { shell }
    } else {
        let theta = rand_signed(rng) * now.vertical_range;
        let phi = rand_signed(rng) * now.horizontal_range;
        let (st, ct) = theta.sin_cos();
        let (sp, cp) = phi.sin_cos();
        Vec3::new(st * cp, st * sp, ct)
    };
    (origin + emitter_frame_turn(local), emitter_frame_turn(dir))
}

#[cfg(test)]
pub(crate) mod tests {
    use model::{CellRamp, EmitParams, EmitTiming, OverLife, ParticleBlend, SplineData};

    use super::*;

    pub(crate) fn now() -> ParamsNow {
        ParamsNow {
            emission_speed: 1.0,
            speed_variation: 0.0,
            vertical_range: 0.5,
            horizontal_range: std::f32::consts::PI,
            gravity: 0.0,
            lifespan: 1.0,
            area_length: 2.0,
            area_width: 4.0,
            z_source: 0.0,
        }
    }

    pub(crate) fn def(shape: ParticleShape) -> ParticleEmitterDef {
        ParticleEmitterDef {
            flags: 0,
            position: [1.0, 2.0, 3.0],
            bone: 0,
            shape,
            blend: ParticleBlend::Add,
            lit: false,
            texture: None,
            tile_rows: 1,
            tile_cols: 1,
            head_tail: 0,
            timing: EmitTiming::constant(10.0),
            params: EmitParams::constant(now()),
            drag: 0.0,
            tail_time: 0.0,
            spline: None,
            geometry_model: None,
            recursion_model: None,
            angular_velocity_min: [0.0; 3],
            angular_velocity_max: [0.0; 3],
            inherit_scale: 0.0,
            follow_speed1: 0.0,
            follow_scale1: 0.0,
            follow_speed2: 0.0,
            follow_scale2: 0.0,
            twinkle_speed: 0.0,
            twinkle_percent: 1.0,
            twinkle_min: 0.0,
            twinkle_max: 0.0,
            spin: 0.0,
            over_life: OverLife {
                mid: 0.5,
                color: [[1.0; 4]; 3],
                scale: [1.0; 3],
                head_cells: [CellRamp::new(0, 0); 2],
                tail_cells: [CellRamp::new(0, 0); 2],
                repeat: [1.0; 2],
            },
        }
    }

    #[test]
    fn a_plane_births_in_its_rectangle_turned_a_quarter() {
        let d = def(ParticleShape::Plane);
        let n = now();
        let mut rng = 12345u32;
        let (mut neg, mut pos) = (false, false);
        let (mut max_dx, mut max_dy) = (0.0f32, 0.0f32);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert!((p.x - 1.0).abs() <= 2.0 + 1e-4 && (p.y - 2.0).abs() <= 1.0 + 1e-4);
            assert!((p.z - 3.0).abs() < f32::EPSILON);
            assert!(dir.z > 0.0);
            max_dx = max_dx.max((p.x - 1.0).abs());
            max_dy = max_dy.max((p.y - 2.0).abs());
            neg |= dir.x < -1e-3;
            pos |= dir.x > 1e-3;
        }
        assert!(neg && pos, "the cone is symmetric");
        assert!(max_dx > 1.5, "the width lands on x");
        assert!(max_dy <= 1.0 + 1e-4, "the length lands on y");
    }

    #[test]
    fn a_sphere_births_on_its_shell_flying_out() {
        let d = def(ParticleShape::Sphere);
        let n = now();
        let mut rng = 99u32;
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let rel = p - Vec3::new(1.0, 2.0, 3.0);
            assert!((2.0 - 1e-3..=4.0 + 1e-3).contains(&rel.length()));
            assert!(rel.normalize().dot(dir) > 0.999);
        }
    }

    #[test]
    fn a_sphere_of_radius_zero_still_sprays() {
        let d = def(ParticleShape::Sphere);
        let mut n = now();
        n.area_length = 0.0;
        n.area_width = 0.0;
        n.vertical_range = std::f32::consts::PI;
        n.horizontal_range = 0.0;
        let mut rng = 4242u32;
        let (mut up, mut down, mut fwd, mut back) = (false, false, false, false);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert_eq!(p, Vec3::new(1.0, 2.0, 3.0));
            assert!((dir.length() - 1.0).abs() < 1e-4 && dir.x.abs() < f32::EPSILON);
            up |= dir.z > 0.5;
            down |= dir.z < -0.5;
            fwd |= dir.y > 0.5;
            back |= dir.y < -0.5;
        }
        assert!(up && down && fwd && back);
    }

    #[test]
    fn a_spline_births_on_its_curve() {
        let x = |v: f32| [v, 0.0, 0.0];
        let mut d = def(ParticleShape::Spline);
        d.spline = SplineData::new(vec![x(0.0), x(1.0), x(2.0), x(3.0)]);
        let mut n = now();
        n.area_length = 0.25;
        n.area_width = 0.75;
        n.vertical_range = 0.0;
        let mut rng = 31u32;
        for _ in 0..64 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let local = p - Vec3::new(1.0, 2.0, 3.0);
            assert!((0.75 - 1e-3..=2.25 + 1e-3).contains(&local.y));
            assert!(local.x.abs() < f32::EPSILON && local.z.abs() < f32::EPSILON);
            assert_eq!(dir, Vec3::ZERO, "no spin: the particle stands");
        }
        n.vertical_range = 1.0;
        n.horizontal_range = 0.5;
        let (mut low, mut high) = (false, false);
        for _ in 0..256 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            assert!(dir.y.abs() < 1e-4 && dir.z > 0.0);
            low |= dir.x > 0.5;
            high |= dir.x < -0.5;
            let local = p - Vec3::new(1.0, 2.0, 3.0);
            assert!(local.x.hypot(local.z) <= 0.5 + 1e-3);
        }
        assert!(low && high);
    }

    #[test]
    fn a_z_source_aims_births_away_from_it() {
        let d = def(ParticleShape::Plane);
        let mut n = now();
        n.z_source = -1.0;
        let mut rng = 7u32;
        for _ in 0..64 {
            let (p, dir) = emit_local(&d, &n, &mut rng);
            let rel = (p - Vec3::new(1.0, 2.0, 3.0)) - Vec3::new(0.0, 0.0, -1.0);
            assert!(rel.normalize().dot(dir) > 0.999);
        }
    }
}
