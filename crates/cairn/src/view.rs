use bevy::prelude::*;
use world::coords::wow_to_bevy;

pub const HUMAN_START: Vec3 = Vec3::new(-8949.95, -132.49, 83.53);
const START_CAMERA_ELEVATION_DEG: f32 = 12.0;
const START_CAMERA_DISTANCE_YD: f32 = 16.0;

/// `eye` and `target` are in WoW's world coordinates; `heading` is radians from north toward
/// west, `pitch` radians, up positive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub eye: Vec3,
    pub heading: f32,
    pub pitch: f32,
    pub target: Vec3,
}

impl Pose {
    /// Standing at `eye`, looking at `at`. Straight up or down, the heading is north.
    pub fn look(eye: Vec3, at: Vec3) -> Self {
        let to = at - eye;
        Self {
            eye,
            heading: to.y.atan2(to.x),
            pitch: to.z.atan2(to.x.hypot(to.y)),
            target: at,
        }
    }

    /// Looking at `at` from `dist` yards out, heading `az_deg` (0 north, 90 west) and raised
    /// `el_deg` above it.
    pub fn orbit(at: Vec3, az_deg: f32, el_deg: f32, dist: f32) -> Self {
        let (az, el) = (az_deg.to_radians(), el_deg.to_radians());
        let back = Vec3::new(
            -ops::cos(el) * ops::cos(az),
            -ops::cos(el) * ops::sin(az),
            ops::sin(el),
        );
        Self {
            eye: at + dist * back,
            heading: az,
            pitch: -el,
            target: at,
        }
    }

    pub fn start(feet: Vec3, facing_deg: f32) -> Self {
        Self::orbit(
            feet,
            facing_deg,
            START_CAMERA_ELEVATION_DEG,
            START_CAMERA_DISTANCE_YD,
        )
    }

    pub fn human_start() -> Self {
        Self::start(HUMAN_START, 0.0)
    }

    pub fn transform(self) -> Transform {
        Transform::from_translation(wow_to_bevy(self.eye.to_array())).with_rotation(
            Quat::from_euler(EulerRot::YXZ, self.heading, self.pitch, 0.0),
        )
    }
}

pub fn camera(pose: Pose) -> impl Bundle {
    world::world_camera(pose.transform())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn heading_zero_sits_south_and_looks_north() {
        let pose = Pose::orbit(Vec3::new(100.0, 50.0, 10.0), 0.0, 0.0, 20.0);
        assert!(close(pose.eye, Vec3::new(80.0, 50.0, 10.0)));
        assert!(close(
            *pose.transform().forward(),
            wow_to_bevy([1.0, 0.0, 0.0])
        ));
    }

    #[test]
    fn heading_ninety_looks_west_and_elevation_looks_down() {
        let pose = Pose::orbit(Vec3::ZERO, 90.0, 30.0, 10.0);
        let el = 30f32.to_radians();
        assert!(close(pose.eye, Vec3::new(0.0, -10.0 * ops::cos(el), 5.0)));
        let down_west = [0.0, ops::cos(el), -ops::sin(el)];
        assert!(close(*pose.transform().forward(), wow_to_bevy(down_west)));
    }

    #[test]
    fn orbit_and_look_agree() {
        let at = Vec3::new(-8949.95, -132.49, 84.0);
        let orbit = Pose::orbit(at, 42.47, 32.5, 48.3);
        let look = Pose::look(orbit.eye, at);
        assert!((orbit.heading - look.heading).abs() < 1e-4);
        assert!((orbit.pitch - look.pitch).abs() < 1e-4);
    }

    #[test]
    fn straight_down_looks_down_with_north_up() {
        let pose = Pose::look(Vec3::new(5.0, 5.0, 100.0), Vec3::new(5.0, 5.0, 0.0));
        let transform = pose.transform();
        assert!(close(*transform.forward(), Vec3::NEG_Y));
        assert!(close(*transform.up(), wow_to_bevy([1.0, 0.0, 0.0])));
    }
}
