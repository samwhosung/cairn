//! The camera's water corridor. Colliding the camera with the waterline has a second half: the
//! framing pivot is re-based against the surface before the boom is built from it, so the boom
//! never starts on the plane it may now hit.

const REST: f32 = 5.0 / 6.0;
const SURFACE_BAND: f32 = 2.0 / 9.0;
const SUBMERGE_EDGE: f32 = 5.0 / 9.0;
const MIN_HEADROOM: f32 = 1.0 / 9.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WaterBand {
    Clear,
    Surface,
    Submerge,
}

/// Against the pivot's target, not its live value, which would chatter on the band edges.
pub fn classify(surface_y: Option<f32>, feet_y: f32, target: f32) -> (WaterBand, f32) {
    let Some(surface_y) = surface_y else {
        return (WaterBand::Clear, 0.0);
    };
    let d = surface_y - feet_y;
    let excess = d - target;
    let band = if excess < SURFACE_BAND {
        WaterBand::Surface
    } else if excess < SUBMERGE_EDGE {
        WaterBand::Submerge
    } else {
        WaterBand::Clear
    };
    (band, d)
}

#[derive(Clone, Copy, Debug)]
pub struct Corridor {
    pub floor: f32,
    pub cap: f32,
}

pub fn corridor(band: WaterBand, d: f32, live: f32) -> Corridor {
    match band {
        WaterBand::Surface => {
            let floor = d + SURFACE_BAND;
            Corridor {
                floor,
                cap: live.max(floor),
            }
        }
        WaterBand::Submerge => Corridor {
            floor: REST,
            cap: (d - REST).max(REST),
        },
        WaterBand::Clear => dry_corridor(live),
    }
}

pub fn dry_corridor(live: f32) -> Corridor {
    Corridor {
        floor: REST,
        cap: live,
    }
}

pub fn pivot_height(c: Corridor, target: f32, live: f32) -> f32 {
    let reach = (target.max(live) - c.floor).max(MIN_HEADROOM);
    (c.floor + reach).min(c.cap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::camera_channel::assert_bounded_step;

    const SWIM_PIVOT: f32 = 1.512_012;
    const SURFACE_DEPTH: f32 = 0.75 * 2.031;

    fn pivot_over_surface(d: f32, target: f32, live: f32) -> f32 {
        let (band, dd) = classify(Some(d), 0.0, target);
        pivot_height(corridor(band, dd, live), target, live) - d
    }

    #[test]
    fn the_surface_band_lifts_the_pivot_clear_of_the_water() {
        let over = pivot_over_surface(SURFACE_DEPTH, SWIM_PIVOT, SWIM_PIVOT);
        assert!((over - SURFACE_BAND).abs() < 1e-5, "{over:+}");
    }

    #[test]
    fn a_surface_swimmer_sits_far_inside_the_band() {
        let excess = SURFACE_DEPTH - SWIM_PIVOT;
        assert!((excess - 0.011_238).abs() < 1e-4);
        assert_eq!(
            classify(Some(SURFACE_DEPTH), 0.0, SWIM_PIVOT).0,
            WaterBand::Surface
        );
        assert!(SURFACE_BAND - excess > 0.2);
    }

    #[test]
    fn diving_into_the_submerge_band_drops_the_pivot_nineteen_eighteenths() {
        let edge = SWIM_PIVOT + SURFACE_BAND;
        let above = pivot_over_surface(edge - 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        let below = pivot_over_surface(edge + 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        assert!((above - SURFACE_BAND).abs() < 1e-3);
        assert!((below + REST).abs() < 1e-3);
        assert!((below - above + 19.0 / 18.0).abs() < 1e-3);
    }

    #[test]
    fn leaving_the_submerge_band_steps_five_eighteenths() {
        let edge = SWIM_PIVOT + SUBMERGE_EDGE;
        let inside = pivot_over_surface(edge - 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        let outside = pivot_over_surface(edge + 1e-4, SWIM_PIVOT, SWIM_PIVOT);
        assert!((outside - inside - 5.0 / 18.0).abs() < 1e-3);
    }

    #[test]
    fn the_only_jump_in_the_whole_range_is_the_dive() {
        let t = SWIM_PIVOT;
        assert_bounded_step((t - 1.5, t + SURFACE_BAND - 0.01), 0.001, 0.002, |d| {
            pivot_over_surface(d, t, t)
        });
        assert_bounded_step((t - 1.5, t + 4.0), 0.0005, 19.0 / 18.0 + 1e-3, |d| {
            pivot_over_surface(d, t, t)
        });
    }

    #[test]
    fn the_option_off_is_dry_land_and_a_nan_depth_stays_finite() {
        let off = dry_corridor(SWIM_PIVOT);
        let dry = corridor(WaterBand::Clear, 3.0, SWIM_PIVOT);
        assert_eq!((off.floor, off.cap), (dry.floor, dry.cap));
        assert_eq!(classify(None, 0.0, SWIM_PIVOT), (WaterBand::Clear, 0.0));
        let (band, d) = classify(Some(f32::NAN), 0.0, SWIM_PIVOT);
        assert_eq!(band, WaterBand::Clear);
        assert!(pivot_height(corridor(band, d, SWIM_PIVOT), SWIM_PIVOT, SWIM_PIVOT).is_finite());
    }
}
