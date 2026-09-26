use bevy::prelude::*;
use world::coords::wow_to_bevy;

pub const HUMAN_START: Vec3 = Vec3::new(-8949.95, -132.49, 83.53);
const START_CAMERA_ELEVATION_DEG: f32 = 12.0;
const START_CAMERA_DISTANCE_YD: f32 = 16.0;
const STEEPEST_WITH_A_HEADING_DEG: f32 = 89.9;

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

    /// The way the camera looks, a unit vector in WoW's axes.
    pub fn forward(self) -> Vec3 {
        let (h, p) = (self.heading, self.pitch);
        Vec3::new(
            ops::cos(p) * ops::cos(h),
            ops::cos(p) * ops::sin(h),
            ops::sin(p),
        )
    }

    pub fn transform(self) -> Transform {
        Transform::from_translation(wow_to_bevy(self.eye.to_array())).with_rotation(
            Quat::from_euler(EulerRot::YXZ, self.heading, self.pitch, 0.0),
        )
    }
}

/// A camera as its flags give it, so the flags it prints give the same pose to the last bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aim {
    Look {
        eye: Vec3,
        at: Vec3,
    },
    Orbit {
        at: Vec3,
        az_deg: f32,
        el_deg: f32,
        dist: f32,
    },
}

impl Aim {
    pub fn start(feet: Vec3, facing_deg: f32) -> Self {
        Self::Orbit {
            at: feet,
            az_deg: facing_deg,
            el_deg: START_CAMERA_ELEVATION_DEG,
            dist: START_CAMERA_DISTANCE_YD,
        }
    }

    pub fn human_start() -> Self {
        Self::start(HUMAN_START, 0.0)
    }

    pub fn pose(self) -> Pose {
        match self {
            Self::Look { eye, at } => Pose::look(eye, at),
            Self::Orbit {
                at,
                az_deg,
                el_deg,
                dist,
            } => Pose::orbit(at, az_deg, el_deg, dist),
        }
    }

    /// Moved along the view, to its left and straight up, looking the same way.
    pub fn moved(self, forward: f32, left: f32, up: f32) -> Self {
        let pose = self.pose();
        let h = pose.heading;
        let leftward = Vec3::new(-ops::sin(h), ops::cos(h), 0.0);
        let by = pose.forward() * forward + leftward * left + Vec3::Z * up;
        Self::Look {
            eye: pose.eye + by,
            at: pose.target + by,
        }
    }

    /// Turned where it stands, the point it looks at as far away as before.
    pub fn turned(self, left_deg: f32, up_deg: f32) -> Self {
        let pose = self.pose();
        let steepest = STEEPEST_WITH_A_HEADING_DEG.to_radians();
        let turned = Pose {
            heading: pose.heading + left_deg.to_radians(),
            pitch: (pose.pitch + up_deg.to_radians()).clamp(-steepest, steepest),
            ..pose
        };
        Self::Look {
            eye: pose.eye,
            at: pose.eye + turned.forward() * pose.eye.distance(pose.target),
        }
    }

    /// Swung round the point it looks at, to its own left and up, and brought `closer` to it.
    pub fn orbited(self, left_deg: f32, up_deg: f32, closer: f32) -> Result<Self, String> {
        let (at, az_deg, el_deg, dist) = match self {
            Self::Orbit {
                at,
                az_deg,
                el_deg,
                dist,
            } => (at, az_deg, el_deg, dist),
            Self::Look { eye, at } => {
                let pose = self.pose();
                let (az, el) = (pose.heading.to_degrees(), -pose.pitch.to_degrees());
                (at, az, el, eye.distance(at))
            }
        };
        let dist = dist - closer;
        if dist <= 0.0 {
            return Err(format!(
                "the camera is {:.2} yd from the point it looks at",
                dist + closer
            ));
        }
        Ok(Self::Orbit {
            at,
            az_deg: (az_deg - left_deg).rem_euclid(360.0),
            el_deg: (el_deg + up_deg)
                .clamp(-STEEPEST_WITH_A_HEADING_DEG, STEEPEST_WITH_A_HEADING_DEG),
            dist,
        })
    }

    pub fn flags(self) -> String {
        let xyz = |v: Vec3| format!("{},{},{}", v.x, v.y, v.z);
        match self {
            Self::Look { eye, at } => format!("--eye {} --look {}", xyz(eye), xyz(at)),
            Self::Orbit {
                at,
                az_deg,
                el_deg,
                dist,
            } => format!("--at {} --az {az_deg} --el {el_deg} --dist {dist}", xyz(at)),
        }
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
    fn a_cameras_flags_give_it_again_to_the_last_bit() {
        for aim in [
            Aim::Look {
                eye: Vec3::new(-9413.333, -80.000_01, 41.789),
                at: Vec3::new(0.1, 1e-7, -3.3),
            },
            Aim::Orbit {
                at: Vec3::new(-8949.95, -132.49, 83.53),
                az_deg: 359.999_97,
                el_deg: -12.34567,
                dist: 0.3,
            },
        ] {
            let flags = aim.flags();
            let words: Vec<&str> = flags.split_whitespace().collect();
            assert_eq!(crate::args::parse_aim(&words), Ok(aim), "{flags}");
        }
    }

    #[test]
    fn a_camera_moves_along_its_view_to_its_left_and_up() {
        let north = Aim::Look {
            eye: Vec3::ZERO,
            at: Vec3::new(10.0, 0.0, 0.0),
        };
        let Aim::Look { eye, at } = north.moved(5.0, 2.0, 3.0) else {
            panic!("a moved camera looks at a point");
        };
        assert!(close(eye, Vec3::new(5.0, 2.0, 3.0)), "{eye}");
        assert!(close(at, Vec3::new(15.0, 2.0, 3.0)), "{at}");
    }

    #[test]
    fn a_camera_turns_where_it_stands() {
        let north = Aim::Look {
            eye: Vec3::ONE,
            at: Vec3::new(11.0, 1.0, 1.0),
        };
        let Aim::Look { eye, at } = north.turned(90.0, 0.0) else {
            panic!("a turned camera looks at a point");
        };
        assert_eq!(eye, Vec3::ONE);
        assert!(close(at, Vec3::new(1.0, 11.0, 1.0)), "west: {at}");
        let up = north.turned(0.0, 30.0).pose();
        assert!((up.pitch - 30f32.to_radians()).abs() < 1e-5);
        let straight_up = north.turned(0.0, 120.0).pose();
        assert!(
            straight_up.pitch < 90f32.to_radians(),
            "never quite straight up"
        );
    }

    #[test]
    fn a_camera_swings_round_the_point_it_looks_at() {
        let south = Aim::Orbit {
            at: Vec3::ZERO,
            az_deg: 0.0,
            el_deg: 0.0,
            dist: 10.0,
        };
        let west = south.orbited(90.0, 0.0, 0.0).expect("a camera").pose();
        assert!(close(west.eye, Vec3::new(0.0, 10.0, 0.0)), "{}", west.eye);
        assert_eq!(west.target, Vec3::ZERO);
        let raised = south.orbited(0.0, 30.0, 4.0).expect("a camera").pose();
        assert!((raised.eye.length() - 6.0).abs() < 1e-5);
        assert!(
            (raised.pitch + 30f32.to_radians()).abs() < 1e-5,
            "looks down"
        );
        assert!(south.orbited(0.0, 0.0, 10.0).is_err());
        let from_look = Aim::Look {
            eye: Vec3::new(-10.0, 0.0, 0.0),
            at: Vec3::ZERO,
        };
        let swung = from_look.orbited(90.0, 0.0, 0.0).expect("a camera").pose();
        assert!(close(swung.eye, Vec3::new(0.0, 10.0, 0.0)), "{}", swung.eye);
    }

    #[test]
    fn straight_down_looks_down_with_north_up() {
        let pose = Pose::look(Vec3::new(5.0, 5.0, 100.0), Vec3::new(5.0, 5.0, 0.0));
        let transform = pose.transform();
        assert!(close(*transform.forward(), Vec3::NEG_Y));
        assert!(close(*transform.up(), wow_to_bevy([1.0, 0.0, 0.0])));
    }
}
