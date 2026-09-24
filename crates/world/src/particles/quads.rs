use std::sync::LazyLock;

use bevy::prelude::*;
use model::ParticleEmitterDef;

use super::Particle;
use super::emit::{emitter_frame_turn, rand01};
use crate::coords::wow_to_bevy;
use crate::effects::EffectVertex;

/// The client fills a 128-entry table of uniform noise at startup that every particle's twinkle
/// reads; this is one such table, from a fixed seed.
static TWINKLE_LUT: LazyLock<[f32; 128]> = LazyLock::new(|| {
    let mut s = 0xC0FF_EE11u32;
    let mut t = [0.0f32; 128];
    for v in &mut t {
        *v = rand01(&mut s);
    }
    t
});

fn twinkle_noise(twinkle_speed: f32, age: f32, phase: u32) -> f32 {
    let idx = ((twinkle_speed * age).clamp(0.0, 255.0) as u32).wrapping_add(phase) as usize & 0x7f;
    TWINKLE_LUT[idx]
}

/// The client turns a negative spin back for the particles whose pool slot's address has bit 5
/// set; bit 5 of the particle's random `phase` stands in for that address.
pub(super) fn spin_angle(spin: f32, age: f32, phase: u32) -> f32 {
    let angle = spin * age;
    if angle < 0.0 && phase & 0x20 != 0 {
        -angle
    } else {
        angle
    }
}

pub(super) struct CamBasis {
    pub(super) right: Vec3,
    pub(super) up: Vec3,
}

pub(super) struct DrawFrame {
    pub(super) world_space: bool,
    pub(super) alpha: f32,
}

pub(super) fn particle_center(frame: &DrawFrame, placement: &Transform, p: &Particle) -> Vec3 {
    if frame.world_space {
        p.pos
    } else {
        placement.transform_point(wow_to_bevy([p.pos.x, p.pos.y, p.pos.z]))
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn expand_quads(
    def: &ParticleEmitterDef,
    particles: &[Particle],
    frame: &DrawFrame,
    placement: &Transform,
    cam: &CamBasis,
    out: &mut Vec<EffectVertex>,
) {
    let world_space = frame.world_space;
    let (cam_right, cam_up) = (cam.right, cam.up);
    let scale = if def.scale_size_by_instance() {
        placement.scale.x.max(1e-4)
    } else {
        1.0
    };
    let plane_basis = def.xy_quad().then(|| {
        let s = placement.scale.x.max(1e-4);
        let axis =
            |v: Vec3| placement.rotation * (wow_to_bevy(emitter_frame_turn(v).to_array()) * s);
        (axis(Vec3::X), axis(Vec3::Y))
    });
    // The column wraps and the row does not: a cell past the atlas's last reads its first row
    // through the sampler's repeat, as the client's does.
    let (cols, rows) = (def.tile_cols, def.tile_rows);
    let (inv_cols, inv_rows) = (1.0 / f32::from(cols), 1.0 / f32::from(rows));
    let cell_uv = |idx: u16| {
        let cx = f32::from(idx & (cols - 1));
        let cy = f32::from(idx >> cols.trailing_zeros());
        (
            (cx * inv_cols, (cx + 1.0) * inv_cols),
            (cy * inv_rows, (cy + 1.0) * inv_rows),
        )
    };
    for p in particles {
        let noise = twinkle_noise(def.twinkle_speed, p.age, p.phase);
        if def.twinkle_percent < 1.0 && noise > def.twinkle_percent {
            continue;
        }
        let u_age = (p.age / p.life).clamp(0.0, 1.0);
        let ol = def.over_life.sample(u_age);
        let (mut rgba, size) = (ol.color, ol.size);
        rgba[3] *= frame.alpha;
        let center = particle_center(frame, placement, p);
        let half = size * def.twinkle(noise) * scale;
        let mut push_quad = |corners: [Vec3; 4], quv: [[f32; 2]; 4]| {
            for (c, t) in corners.iter().zip(quv) {
                out.push(EffectVertex {
                    pos: c.to_array(),
                    uv: t,
                    color: rgba,
                });
            }
        };
        if def.head_tail != 1 {
            let ((u0, u1), (v0, v1)) = cell_uv(ol.head_cell);
            let (base_r, base_u) = plane_basis.unwrap_or((cam_right, cam_up));
            let (r, u) = if def.spin == 0.0 {
                (base_r * half, base_u * half)
            } else {
                let (sa, ca) = spin_angle(def.spin, p.age, p.phase).sin_cos();
                (
                    (base_r * ca + base_u * sa) * half,
                    (base_u * ca - base_r * sa) * half,
                )
            };
            push_quad(
                [
                    center - r - u,
                    center + r - u,
                    center + r + u,
                    center - r + u,
                ],
                [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
            );
        }
        if def.head_tail >= 1 {
            let ((u0, u1), (v0, v1)) = cell_uv(ol.tail_cell);
            let vel_world = if world_space {
                p.vel
            } else {
                placement.rotation * (placement.scale * wow_to_bevy(p.vel.to_array()))
            };
            let t_eff = if def.tail_clamps_to_age() {
                def.tail_time.min(p.age)
            } else {
                def.tail_time
            };
            let tail = -vel_world * t_eff;
            let (tr, tu) = (tail.dot(cam_right), tail.dot(cam_up));
            let l2 = tr * tr + tu * tu;
            if l2 < 7.7e-4 {
                let (r, u) = (cam_right * half, cam_up * half);
                push_quad(
                    [
                        center - r - u,
                        center + r - u,
                        center + r + u,
                        center - r + u,
                    ],
                    [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
                );
            } else {
                let inv_l = half / l2.sqrt();
                let perp = (cam_up * tr - cam_right * tu) * inv_l;
                push_quad(
                    [
                        center - perp,
                        center + perp,
                        center + tail + perp,
                        center + tail - perp,
                    ],
                    [[u0, v1], [u0, v0], [u1, v0], [u1, v1]],
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_spin_turns_the_bit_5_half_the_other_way() {
        assert_eq!(spin_angle(-3.0, 0.5, 0x20), 1.5);
        assert_eq!(spin_angle(-3.0, 0.5, 0x1f), -1.5);
        assert_eq!(spin_angle(3.0, 0.5, 0x20), 1.5);
        assert_eq!(spin_angle(0.0, 0.5, 0xff), 0.0);
    }

    #[test]
    fn the_models_alpha_reaches_the_alpha_and_nothing_else() {
        let mut def = super::super::emit::tests::def(model::ParticleShape::Plane);
        def.over_life.color = [[0.8, 0.4, 0.2, 0.5]; 3];
        let pool = [Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            age: 0.0,
            life: 1.0,
            phase: 0,
            fresh: false,
            quat: Quat::IDENTITY,
            angvel: Vec3::ZERO,
        }];
        let cam = CamBasis {
            right: Vec3::X,
            up: Vec3::Y,
        };
        let shoot = |alpha| {
            let frame = DrawFrame {
                world_space: true,
                alpha,
            };
            let mut out = Vec::new();
            expand_quads(&def, &pool, &frame, &Transform::IDENTITY, &cam, &mut out);
            out[0].color
        };
        assert_eq!(shoot(1.0), [0.8, 0.4, 0.2, 0.5]);
        let half = shoot(0.5);
        assert_eq!([half[0], half[1], half[2]], [0.8, 0.4, 0.2]);
        assert!((half[3] - 0.25).abs() < 1e-6);
        assert_eq!(shoot(0.0)[3], 0.0);
    }
}
