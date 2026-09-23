const PARAM_SLIME: u32 = 6;
const PARAM_MAGMA: u32 = 7;

const OCEAN_RAMP_FLOOR: f32 = -30.0;
/// The client multiplies by this reciprocal; dividing by 30 rounds differently.
const OCEAN_RAMP_RECIP: f32 = 1.0 / OCEAN_RAMP_FLOOR;

/// What the camera is under, which picks the atmosphere.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Submersion {
    #[default]
    Dry,
    /// Still water or rapids: the zone's underwater profile.
    Water,
    /// The zone's underwater profile, darkened by [`Self::ocean_depth_factors`].
    Ocean,
    /// A fixed profile, whatever the zone.
    Magma,
    /// A fixed profile, whatever the zone.
    Slime,
}

impl Submersion {
    pub fn any(self) -> bool {
        self != Submersion::Dry
    }

    /// Water or ocean.
    pub fn is_water(self) -> bool {
        matches!(self, Submersion::Water | Submersion::Ocean)
    }

    /// Under ocean only, `(ambient, diffuse)`: the factors the client scales those two colours'
    /// HSV value by, for a camera at world height `eye_z`. They fall linearly from 1.0 at height 0
    /// to 0.5 and 0.75 at -30, and hold there.
    pub fn ocean_depth_factors(self, eye_z: f32) -> Option<(f32, f32)> {
        if self != Submersion::Ocean {
            return None;
        }
        let t = eye_z.clamp(OCEAN_RAMP_FLOOR, 0.0);
        let k = 1.0 - t * OCEAN_RAMP_RECIP;
        Some((k.midpoint(1.0), k * 0.25 + 0.75))
    }

    pub(crate) fn fixed_param(self) -> Option<u32> {
        match self {
            Submersion::Magma => Some(PARAM_MAGMA),
            Submersion::Slime => Some(PARAM_SLIME),
            Submersion::Dry | Submersion::Water | Submersion::Ocean => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sample::{SLOT_CLEAR_UNDERWATER, weather_slot};

    #[test]
    fn the_ocean_ramp_runs_over_thirty_yards_and_then_holds() {
        let f = |z: f32| {
            Submersion::Ocean
                .ocean_depth_factors(z)
                .expect("ocean ramps")
        };
        assert_eq!(f(0.0), (1.0, 1.0));
        assert_eq!(f(12.0), (1.0, 1.0));
        let (a30, b30) = f(-30.0);
        assert!((a30 - 0.5).abs() < 1e-6 && (b30 - 0.75).abs() < 1e-6);
        assert_eq!(f(-100.0), (a30, b30));
        let (a15, b15) = f(-15.0);
        assert!((a15 - 0.75).abs() < 1e-6 && (b15 - 0.875).abs() < 1e-6);
        assert!(((1.0 - a15) - 2.0 * (1.0 - b15)).abs() < 1e-6);
    }

    #[test]
    fn the_ramp_multiplies_by_the_stored_reciprocal() {
        assert_eq!(OCEAN_RAMP_RECIP.to_bits(), 0xbd08_8889);
        let t = -29.0f32;
        assert_ne!(
            (1.0 - t * OCEAN_RAMP_RECIP).to_bits(),
            (1.0 + t / 30.0).to_bits()
        );
        assert_eq!(
            Submersion::Ocean.ocean_depth_factors(-30.0),
            Some((0.5, 0.75))
        );
    }

    #[test]
    fn only_ocean_ramps() {
        for s in [
            Submersion::Dry,
            Submersion::Water,
            Submersion::Magma,
            Submersion::Slime,
        ] {
            assert!(s.ocean_depth_factors(-30.0).is_none(), "{s:?}");
        }
    }

    #[test]
    fn ocean_is_water_everywhere_but_the_ramp() {
        assert!(Submersion::Ocean.is_water() && Submersion::Water.is_water());
        assert!(!Submersion::Magma.is_water() && !Submersion::Slime.is_water());
        assert!(Submersion::Ocean.any() && !Submersion::Dry.any());
        assert_eq!(Submersion::Ocean.fixed_param(), None);
        assert_eq!(
            weather_slot(false, false, Submersion::Ocean.is_water()),
            SLOT_CLEAR_UNDERWATER
        );
    }
}
