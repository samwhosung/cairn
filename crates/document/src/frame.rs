//! A zone's own frame: x east and y south in yards from its north-west corner, z a world height.
//! It is its map seen from above, and the ADT placement frame shifted by the zone's first tile.
//! The world frame cairn takes on its command line is x north, y west.

pub const TILE: f64 = 1600.0 / 3.0;
pub const CHUNK: f64 = TILE / 16.0;
pub const CELL: f64 = CHUNK / 8.0;
pub const TEXEL: f64 = CHUNK / 64.0;
/// The map's middle: world x is this less the yards south of the map's north-west corner.
pub const CENTRE: f64 = 32.0 * TILE;
pub const MAP_TILES: u32 = 64;
/// The most tiles a zone of its own spans each way.
pub const MAX_ZONE_TILES: u32 = 8;

/// Where a zone lies on its map: `origin` is the tile of its north-west corner, named as the
/// map's files name it (`<map>_<x>_<y>`), and `size` its tiles east and south.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub origin: (u32, u32),
    pub size: (u32, u32),
}

impl Frame {
    /// Yards east and south.
    pub fn extent(&self) -> [f64; 2] {
        [f64::from(self.size.0) * TILE, f64::from(self.size.1) * TILE]
    }

    /// The world `(x north, y west)` of a zone point.
    pub fn world(&self, p: [f64; 2]) -> [f64; 2] {
        [
            CENTRE - (f64::from(self.origin.1) * TILE + p[1]),
            CENTRE - (f64::from(self.origin.0) * TILE + p[0]),
        ]
    }

    /// Cells east and south; the outer lattice has one more vertex each way.
    pub fn cells(&self) -> (usize, usize) {
        (self.size.0 as usize * 128, self.size.1 as usize * 128)
    }

    pub fn chunks(&self) -> (usize, usize) {
        (self.size.0 as usize * 16, self.size.1 as usize * 16)
    }

    pub fn contains(&self, p: [f64; 2]) -> bool {
        let e = self.extent();
        (0.0..=e[0]).contains(&p[0]) && (0.0..=e[1]).contains(&p[1])
    }

    /// Every tile of the zone as its file names it, row by row.
    pub fn tiles(&self) -> impl Iterator<Item = (u32, u32)> + use<> {
        let (o, s) = (self.origin, self.size);
        (0..s.1).flat_map(move |ty| (0..s.0).map(move |tx| (o.0 + tx, o.1 + ty)))
    }
}

/// The ADT heading (a placement's second rotation) of a compass bearing (0 north, 90 east) of a
/// model's front: a model's +x points along bearing `180 − heading`.
pub fn heading_of(bearing: f64) -> f64 {
    (180.0 - bearing).rem_euclid(360.0)
}

pub fn bearing_vec(bearing_deg: f64) -> [f64; 2] {
    let r = bearing_deg.to_radians();
    [libm::sin(r), -libm::cos(r)]
}

pub fn bearing_of(d: [f64; 2]) -> f64 {
    libm::atan2(d[0], -d[1]).to_degrees().rem_euclid(360.0)
}

pub fn compass(bearing: f64) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[(((bearing.rem_euclid(360.0) + 22.5) / 45.0) as usize) % 8]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_names_the_world_and_faces_the_right_way() {
        let f = Frame {
            origin: (32, 48),
            size: (2, 1),
        };
        let w = f.world([10.0, 20.0]);
        assert!((w[0] - (CENTRE - 48.0 * TILE - 20.0)).abs() < 1e-9);
        assert!((w[1] - (CENTRE - 32.0 * TILE - 10.0)).abs() < 1e-9);
        assert_eq!(f.tiles().collect::<Vec<_>>(), [(32, 48), (33, 48)]);
        assert!((heading_of(90.0) - 90.0).abs() < 1e-9);
        assert!((heading_of(0.0) - 180.0).abs() < 1e-9);
        let east = bearing_vec(90.0);
        assert!((east[0] - 1.0).abs() < 1e-12 && east[1].abs() < 1e-12);
        assert!(bearing_of([0.0, -1.0]).abs() < 1e-9);
        assert_eq!(compass(135.0), "SE");
    }
}
