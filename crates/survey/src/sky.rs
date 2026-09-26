//! A zone's sky at a few hours, read from the lighting tables where its ground is, and drawn as
//! the client paints its dome: colours at five elevations, the fog's at the horizon.

use std::path::Path;

use image::{Rgb, RgbImage};
use light::{Atmosphere, LightCatalog, Submersion};

use crate::font;
use crate::picture::{save_png, write_atomically};

/// The hours a zone's sky is shown at, in game minutes.
pub(crate) const HOURS: [(&str, u32); 4] = [
    ("dawn 06:30", 6 * 60 + 30),
    ("noon 12:00", 12 * 60),
    ("dusk 21:30", 21 * 60 + 30),
    ("midnight 00:00", 0),
];
/// The dome's colours are set at these elevations, zenith first; the fog's lies at 0 and below.
const RINGS: [f32; 5] = [90.0, 16.8, 9.8, 3.7, 1.8];
const PANEL: u32 = 200;
const GUTTER: u32 = 8;
const TITLE: u32 = 18;
const DOME: u32 = 200;
const BELOW: u32 = 30;
const CHIP: u32 = 44;
const CHIP_LABEL: u32 = 12;
const PAGE: [u8; 3] = [38, 40, 44];
const INK: [u8; 3] = [232, 232, 232];

/// The atmosphere at `heart` on `map` at each of [`HOURS`], dry and clear.
pub(crate) fn skies(catalog: &LightCatalog, map: u32, heart: [f32; 3]) -> [Atmosphere; 4] {
    HOURS.map(|(_, minute)| catalog.sample(map, heart, minute * 2, false, Submersion::Dry, false))
}

fn rgb(c: [f32; 3]) -> [u8; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// `#rrggbb`.
pub(crate) fn hex(c: [f32; 3]) -> String {
    let [r, g, b] = rgb(c);
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn dome_at(a: &Atmosphere, elevation: f32) -> [f32; 3] {
    if elevation <= 0.0 {
        return a.fog_color;
    }
    let mix = |p: [f32; 3], q: [f32; 3], t: f32| std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t);
    if elevation < RINGS[4] {
        return mix(a.fog_color, a.sky[4], elevation / RINGS[4]);
    }
    for i in (0..4).rev() {
        if elevation < RINGS[i] {
            let t = (elevation - RINGS[i + 1]) / (RINGS[i] - RINGS[i + 1]);
            return mix(a.sky[i + 1], a.sky[i], t);
        }
    }
    a.sky[0]
}

/// Draws the four hours side by side: the dome from the zenith down to the horizon, the fog below
/// it, and chips of the sun's and the ambient light and of the water.
pub(crate) fn draw(title: &str, skies: &[Atmosphere; 4], out: &Path) -> Result<(), String> {
    let chips = |a: &Atmosphere| {
        [
            ("SUN", a.sun_diffuse),
            ("AMBIENT", a.ambient),
            ("RIVER", a.water_river[0]),
            ("DEEP", a.water_river[1]),
            ("OCEAN", a.water_ocean[0]),
            ("DEEP", a.water_ocean[1]),
        ]
    };
    let rows = 2;
    let width = GUTTER + HOURS.len() as u32 * (PANEL + GUTTER);
    let height = TITLE + DOME + BELOW + GUTTER + rows * (CHIP + CHIP_LABEL) + GUTTER;
    let mut img = RgbImage::from_pixel(width, height, Rgb(PAGE));
    font::draw(&mut img, (GUTTER, 5), title, 1, 200, INK);
    for (i, ((name, _), a)) in HOURS.iter().zip(skies).enumerate() {
        let left = GUTTER + i as u32 * (PANEL + GUTTER);
        let top = TITLE;
        for r in 0..DOME + BELOW {
            let t = (r as f32 + 0.5) / DOME as f32;
            let elevation = if r < DOME {
                90.0 * (1.0 - t) * (1.0 - t)
            } else {
                -1.0
            };
            let c = Rgb(rgb(dome_at(a, elevation)));
            for x in left..left + PANEL {
                img.put_pixel(x, top + r, c);
            }
        }
        font::draw(&mut img, (left + 4, top + 4), name, 1, 30, INK);
        for (k, (label, color)) in chips(a).into_iter().enumerate() {
            let per_row = 3;
            let (row, col) = (k as u32 / per_row, k as u32 % per_row);
            let w = (PANEL - (per_row - 1) * 4) / per_row;
            let x = left + col * (w + 4);
            let y = top + DOME + BELOW + GUTTER + row * (CHIP + CHIP_LABEL);
            let c = Rgb(rgb(color));
            for yy in y..y + CHIP {
                for xx in x..x + w {
                    img.put_pixel(xx, yy, c);
                }
            }
            font::draw(&mut img, (x, y + CHIP + 3), label, 1, 10, INK);
        }
    }
    write_atomically(out, |part| save_png(&img, part))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dome_is_the_fog_below_the_horizon_and_the_zenith_on_top() {
        let a = Atmosphere {
            fog_color: [0.0, 0.0, 1.0],
            sky: [
                [1.0, 0.0, 0.0],
                [0.8, 0.0, 0.0],
                [0.6, 0.0, 0.0],
                [0.4, 0.0, 0.0],
                [0.2, 0.0, 0.0],
            ],
            ..Atmosphere::DEFAULT
        };
        let at = |elevation: f32| hex(dome_at(&a, elevation));
        assert_eq!(at(-3.0), "#0000ff", "the fog below the horizon");
        assert_eq!(at(90.0), "#ff0000", "the zenith");
        assert_eq!(at(16.8), "#cc0000", "the second ring");
        assert_eq!(
            at(0.9),
            "#1a0080",
            "halfway from the fog to the lowest ring"
        );
    }
}
