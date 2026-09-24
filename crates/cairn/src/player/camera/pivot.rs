//! The framing pivot the camera looks at, and the body's fade as the camera closes on it.

use bevy::prelude::*;

use super::super::camera_channel::{Arm, SmoothChannel};

/// The pivot's floor and ceiling above the feet, yd.
pub const CAM_PIVOT_FLOOR: f32 = 5.0 / 6.0;
pub const CAM_PIVOT_CEIL: f32 = 15.0;
/// The pivot channel's average rate, yd/s.
const CAM_PIVOT_SMOOTH_SPEED: f32 = 1.2;
/// Over this distance beyond the near clip the body fades in, yd.
pub const SELF_FADE_WINDOW: f32 = 1.8315;
/// Closer than this beyond the near clip the body is hidden: first person.
const SELF_FADE_HIDE: f32 = 0.00278;

/// A model's pivot height in its own yards: the head attachment's height, and how far it drops
/// while swimming.
#[derive(Component, Clone, Copy, Debug)]
pub struct CameraPivot {
    pub height_local: f32,
    pub swim_drop_local: f32,
}

impl CameraPivot {
    /// No pivot of its own: the camera sits at the floor.
    pub const FLOOR: Self = Self {
        height_local: 0.0,
        swim_drop_local: 0.0,
    };

    pub fn of(model: Option<&world::M2Model>) -> Self {
        model
            .and_then(|m| m.bounds.as_ref())
            .map_or(Self::FLOOR, |b| Self {
                height_local: b.pivot_z.map_or_else(
                    || PIVOT_SHARE_OF_BOX * (b.bbox_max[2] - b.bbox_min[2]).max(0.0),
                    |z| z + PIVOT_ABOVE_ATTACHMENT,
                ),
                swim_drop_local: b.swim_pivot_drop,
            })
    }
}

const PIVOT_ABOVE_ATTACHMENT: f32 = 0.0972;
const PIVOT_SHARE_OF_BOX: f32 = 0.9;

pub fn model_pivot_height(pivot: CameraPivot, scale: f32, swimming: bool) -> f32 {
    let local = if swimming {
        pivot.height_local - pivot.swim_drop_local
    } else {
        pivot.height_local
    };
    (local * scale).clamp(CAM_PIVOT_FLOOR, CAM_PIVOT_CEIL)
}

/// The pivot's height channel. The first height a camera sees is established, not travelled to;
/// every change after glides. With no target it holds.
#[derive(Default)]
pub struct PivotGlide {
    channel: SmoothChannel,
    seeded: bool,
}

impl PivotGlide {
    pub fn advance(&mut self, target: Option<f32>, dt: f32) -> f32 {
        if let Some(target) = target {
            if self.seeded {
                self.channel.arm(&Arm::at(target, CAM_PIVOT_SMOOTH_SPEED));
            } else {
                self.seeded = true;
                self.channel.snap(target);
            }
        }
        self.channel.advance(dt)
    }

    pub fn target(&self) -> f32 {
        self.channel.target()
    }
}

pub fn self_model_fade_alpha(dist: f32, nearclip: f32, window: f32) -> f32 {
    let d = dist - nearclip;
    if d <= SELF_FADE_HIDE {
        return 0.0;
    }
    if d >= window {
        return 1.0;
    }
    0.5 * (1.0 - ops::cos(std::f32::consts::PI * d / window))
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::super::super::camera_channel::CHANNEL_EPS;
    use super::*;

    fn glide_run(g: &mut PivotGlide, target: Option<f32>, secs: f32) -> Vec<f32> {
        let dt = 1.0 / 60.0;
        (0..(secs / dt).round() as usize)
            .map(|_| g.advance(target, dt))
            .collect()
    }

    #[test]
    fn a_change_of_body_glides_the_pivot_both_ways() {
        let (tauren, cat) = (2.4659_f32, 1.0552_f32);
        let mut g = PivotGlide::default();
        assert_eq!(
            g.advance(Some(tauren), 1.0 / 60.0),
            tauren,
            "the first arm snaps"
        );
        for (from, to) in [(tauren, cat), (cat, tauren)] {
            let expected = (to - from).abs() / CAM_PIVOT_SMOOTH_SPEED;
            let frames = glide_run(&mut g, Some(to), expected * 2.0);
            let arrived = frames
                .iter()
                .position(|h| (h - to).abs() < CHANNEL_EPS)
                .expect("arrives");
            assert!((arrived as f32 / 60.0 - expected).abs() < 0.05);
            let biggest = frames
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max);
            assert!(biggest < (to - from).abs() * 0.5);
        }
    }

    #[test]
    fn a_body_with_no_model_holds_the_pivot() {
        let mut g = PivotGlide::default();
        g.advance(Some(2.4659), 1.0 / 60.0);
        assert!(glide_run(&mut g, None, 0.5).iter().all(|h| *h == 2.4659));
        let frames = glide_run(&mut g, Some(1.0552), 2.0);
        assert!(frames[0] < 2.4659 && frames[0] > 2.4);
    }

    #[test]
    fn the_pivot_is_clamped_and_drops_while_swimming() {
        let p = CameraPivot {
            height_local: 2.0,
            swim_drop_local: 0.0,
        };
        assert_eq!(model_pivot_height(p, 1.0, false), 2.0);
        assert_eq!(model_pivot_height(p, 0.01, false), CAM_PIVOT_FLOOR);
        assert_eq!(model_pivot_height(p, 100.0, false), CAM_PIVOT_CEIL);
        let human = CameraPivot {
            height_local: 1.900_269_2,
            swim_drop_local: 0.388_257_2,
        };
        assert!((model_pivot_height(human, 1.0, false) - 1.900_269_2).abs() < 1e-5);
        assert!((model_pivot_height(human, 1.0, true) - 1.512_012).abs() < 1e-5);
    }

    #[test]
    fn the_body_hides_at_the_near_plane_and_is_opaque_a_window_out() {
        let nc = 0.1;
        assert_eq!(self_model_fade_alpha(nc, nc, SELF_FADE_WINDOW), 0.0);
        assert_eq!(self_model_fade_alpha(0.0, nc, SELF_FADE_WINDOW), 0.0);
        assert_eq!(
            self_model_fade_alpha(nc + SELF_FADE_HIDE, nc, SELF_FADE_WINDOW),
            0.0
        );
        assert_eq!(
            self_model_fade_alpha(nc + SELF_FADE_WINDOW, nc, SELF_FADE_WINDOW),
            1.0
        );
        let mid = self_model_fade_alpha(nc + SELF_FADE_WINDOW / 2.0, nc, SELF_FADE_WINDOW);
        assert!((mid - 0.5).abs() < 1e-5);
    }
}
