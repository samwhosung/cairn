use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::adt::AdtTile;
use crate::clouds::CloudCoverage;
use crate::ground::terrain_wow_z_under;
use crate::horizon::Horizon;
use crate::room::CameraRoom;
use crate::stream::Streamer;
use crate::submersion::SubmergedEye;

const RAY_SAMPLES: u32 = 48;
const RAY_RANGE: f32 = 2800.0;
const GRID: u32 = 4;
const CELLS: u32 = GRID * GRID;
const RAYS_PER_FRAME: u32 = 2;
const HORIZON_FADE_SIN: f32 = 0.035;
pub(super) const SUN_RISE_PER_SEC: f32 = 4.0;
pub(super) const MOON_RISE_PER_SEC: f32 = 1.0 / 0.33;
const FALL_PER_SEC: f32 = 1.0 / 0.66;

#[derive(Component, Default)]
pub(super) struct Glare {
    brightness: f32,
    clear_cells: u16,
    next_cell: u32,
    primed: bool,
}

#[derive(SystemParam)]
pub(super) struct FlareGate<'w> {
    time: Res<'w, Time>,
    streamer: Res<'w, Streamer>,
    adts: Res<'w, Assets<AdtTile>>,
    horizon: Res<'w, Horizon>,
    room: Res<'w, CameraRoom>,
    submerged: Res<'w, SubmergedEye>,
    pub(super) clouds: Res<'w, CloudCoverage>,
}

pub(super) struct Allowance {
    pub hour: f32,
    pub cloud_clearance: f32,
    pub disc_half_angle: f32,
    pub rise_per_sec: f32,
}

impl FlareGate<'_> {
    pub(super) fn ease(&self, glare: &mut Glare, eye: Vec3, dir: Vec3, allow: &Allowance) -> f32 {
        let base = allow.hour
            * horizon_fade(dir)
            * allow.cloud_clearance
            * submersion_fade(self.submerged.depth);
        let target = if base > 0.0 && !self.room.indoors {
            let (streamer, adts, horizon) = (&*self.streamer, &*self.adts, &*self.horizon);
            let height_under = |p: Vec3| {
                terrain_wow_z_under(streamer, adts, p).or_else(|| horizon.height_under(p))
            };
            let cells = if glare.primed { RAYS_PER_FRAME } else { CELLS };
            glare.primed = true;
            let visible =
                march_visible_fraction(&mut glare.clear_cells, &mut glare.next_cell, cells, |c| {
                    cell_clear(height_under, eye, dir, allow.disc_half_angle, c)
                });
            base * visible
        } else {
            glare.primed = false;
            0.0
        };
        glare.brightness = ease(
            glare.brightness,
            target,
            allow.rise_per_sec,
            FALL_PER_SEC,
            self.time.delta_secs(),
        );
        glare.brightness
    }
}

/// The client fades the glare out over the first ten yards of water over the eye.
fn submersion_fade(depth: f32) -> f32 {
    1.0 - (depth * 0.1).clamp(0.0, 1.0)
}

fn horizon_fade(dir: Vec3) -> f32 {
    let t = (dir.y / HORIZON_FADE_SIN).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease(current: f32, target: f32, rise: f32, fall: f32, dt: f32) -> f32 {
    current + (target - current).clamp(-fall * dt, rise * dt)
}

fn ray_clear(height_under: impl Fn(Vec3) -> Option<f32>, eye: Vec3, dir: Vec3) -> bool {
    (1..=RAY_SAMPLES).all(|i| {
        let s = i as f32 / RAY_SAMPLES as f32;
        let p = eye + dir * (RAY_RANGE * s * s);
        height_under(p).is_none_or(|z| z <= p.y)
    })
}

fn cell_clear(
    height_under: impl Fn(Vec3) -> Option<f32>,
    eye: Vec3,
    dir: Vec3,
    half_angle: f32,
    cell: u32,
) -> bool {
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let right = if right == Vec3::ZERO { Vec3::X } else { right };
    let up = right.cross(dir);
    let u = ((cell / GRID) as f32 + 0.5) / GRID as f32 - 0.5;
    let v = ((cell % GRID) as f32 + 0.5) / GRID as f32 - 0.5;
    let d = (dir + right * (u * 2.0 * half_angle) + up * (v * 2.0 * half_angle)).normalize();
    ray_clear(height_under, eye, d)
}

fn march_visible_fraction(
    clear_cells: &mut u16,
    next_cell: &mut u32,
    cells: u32,
    clear: impl Fn(u32) -> bool,
) -> f32 {
    for _ in 0..cells {
        let c = *next_cell % CELLS;
        if clear(c) {
            *clear_cells |= 1 << c;
        } else {
            *clear_cells &= !(1 << c);
        }
        *next_cell = (c + 1) % CELLS;
    }
    f32::from(clear_cells.count_ones() as u16) / CELLS as f32
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[allow(
        clippy::unnecessary_wraps,
        reason = "a height lookup, which may find nothing"
    )]
    fn ridge_past_300_yards(p: Vec3) -> Option<f32> {
        Some(if Vec2::new(p.x, p.z).length() > 300.0 {
            30.0
        } else {
            0.0
        })
    }

    fn elevated(deg: f32) -> Vec3 {
        let e = deg.to_radians();
        Vec3::new(e.cos(), e.sin(), 0.0)
    }

    fn fraction(eye: Vec3, dir: Vec3, half: f32) -> f32 {
        let (mut clear, mut next) = (0, 0);
        march_visible_fraction(&mut clear, &mut next, CELLS, |c| {
            cell_clear(ridge_past_300_yards, eye, dir, half, c)
        })
    }

    #[test]
    fn easing_is_linear_uneven_and_stops_at_the_target() {
        let (rise, fall) = (SUN_RISE_PER_SEC, FALL_PER_SEC);
        assert!((ease(0.0, 1.0, rise, fall, 0.1) - 0.4).abs() < 1e-6);
        assert!((ease(1.0, 0.0, rise, fall, 0.1) - (1.0 - fall * 0.1)).abs() < 1e-6);
        assert_eq!(ease(0.9, 1.0, rise, fall, 1.0), 1.0);
        assert_eq!(ease(0.1, 0.0, rise, fall, 1.0), 0.0);
        assert_eq!(MOON_RISE_PER_SEC.to_bits(), 0x4041_f07c);
        assert_eq!(FALL_PER_SEC.to_bits(), 0x3fc1_f07c);
    }

    #[test]
    fn a_ridge_hides_a_low_body_and_half_hides_one_on_its_crest() {
        let eye = Vec3::new(0.0, 2.0, 0.0);
        let half = 2.0_f32.to_radians();
        assert!(!ray_clear(ridge_past_300_yards, eye, elevated(0.6)));
        assert!(ray_clear(ridge_past_300_yards, eye, elevated(45.0)));
        assert!(ray_clear(|_| None, eye, elevated(0.6)));
        assert_eq!(fraction(eye, elevated(12.0), half), 1.0);
        assert_eq!(fraction(eye, elevated(1.0), half), 0.0);
        let crest = fraction(eye, elevated(5.4), half);
        assert!((0.25..=0.75).contains(&crest), "{crest}");
    }

    #[test]
    fn marching_a_few_cells_a_frame_comes_to_the_whole_march() {
        let (eye, dir, half) = (
            Vec3::new(0.0, 2.0, 0.0),
            elevated(5.4),
            2.0_f32.to_radians(),
        );
        let whole = fraction(eye, dir, half);
        let (mut clear, mut next) = (0, 0);
        let march = |c| cell_clear(ridge_past_300_yards, eye, dir, half, c);
        let mut last = march_visible_fraction(&mut clear, &mut next, CELLS, march);
        assert_eq!(last, whole);
        for _ in 0..CELLS / RAYS_PER_FRAME {
            last = march_visible_fraction(&mut clear, &mut next, RAYS_PER_FRAME, march);
        }
        assert_eq!((last, next), (whole, 0));
    }
}
