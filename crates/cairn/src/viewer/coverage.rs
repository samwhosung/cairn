use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use bevy::prelude::*;
use world::sight::frame::{Shown, SightIndex};

#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    pub pixels: u32,
    pub sky: u32,
    pub ground: u32,
    /// Pixels whose colour names no placement the frame knew.
    pub unnamed: u32,
    pub placements: BTreeMap<u32, Covered>,
}

#[derive(Debug, PartialEq)]
pub struct Covered {
    pub pixels: u32,
    pub min: UVec2,
    pub max: UVec2,
    /// The sRGB bytes of its pixels in the sight frame.
    pub colour: [u8; 3],
    pub building: bool,
    pub file: Arc<str>,
}

impl Coverage {
    /// Tallies an RGBA8 sight frame `width` pixels across.
    pub fn count(rgba: &[u8], width: u32, index: &SightIndex) -> Self {
        let mut coverage = Self::default();
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let at = UVec2::new(i as u32 % width.max(1), i as u32 / width.max(1));
            coverage.pixels += 1;
            match index.shown([px[0], px[1], px[2]]) {
                Some(Shown::Nothing) => coverage.sky += 1,
                Some(Shown::Ground) => coverage.ground += 1,
                None => coverage.unnamed += 1,
                Some(Shown::Placed(p)) => {
                    let covered =
                        coverage
                            .placements
                            .entry(p.unique_id)
                            .or_insert_with(|| Covered {
                                pixels: 0,
                                min: at,
                                max: at,
                                colour: index.colour(p.unique_id).unwrap_or_default(),
                                building: p.building,
                                file: p.file.clone(),
                            });
                    covered.pixels += 1;
                    covered.min = covered.min.min(at);
                    covered.max = covered.max.max(at);
                }
            }
        }
        coverage
    }

    fn share(&self, pixels: u32) -> String {
        format!(
            "{:.2}%",
            100.0 * f64::from(pixels) / f64::from(self.pixels.max(1))
        )
    }

    pub fn listing(&self, camera: &str, size: UVec2, left_out: &[u32]) -> String {
        let mut text = String::from(
            "# the placements a camera saw, each by the unique id the map's files place it under: the\n\
             # share of the frame's pixels it covers, how many, the box they lie in, x,y from the\n\
             # frame's top left, and the colour of its pixels in the frame beside this list that\n\
             # names each pixel's placement. It covers a pixel where it is the nearest thing drawn: a\n\
             # see-through part where at least half of it shows, and never a part that only lights or\n\
             # shades what lies behind. A building's own doodads count as the building; sky is\n\
             # whatever lies past the far clip.\n",
        );
        let _ = writeln!(text, "camera {camera}");
        let _ = writeln!(text, "frame {}x{}", size.x, size.y);
        let _ = writeln!(
            text,
            "ground {} {}",
            self.share(self.ground),
            hex(SightIndex::ground_colour())
        );
        let _ = writeln!(text, "sky {} {}", self.share(self.sky), hex([0; 3]));
        if self.unnamed > 0 {
            let _ = writeln!(text, "unnamed {}", self.share(self.unnamed));
        }
        if !left_out.is_empty() {
            let ids: Vec<String> = left_out.iter().map(u32::to_string).collect();
            let _ = writeln!(text, "left-out {}", ids.join(" "));
        }
        let _ = writeln!(text, "id share pixels box colour kind model");
        for (id, c) in self.largest_first() {
            let kind = if c.building { "building" } else { "doodad" };
            let _ = writeln!(
                text,
                "{id} {} {} {},{}-{},{} {} {kind} {}",
                self.share(c.pixels),
                c.pixels,
                c.min.x,
                c.min.y,
                c.max.x,
                c.max.y,
                hex(c.colour),
                c.file
            );
        }
        text
    }

    fn largest_first(&self) -> Vec<(u32, &Covered)> {
        let mut by: Vec<(u32, &Covered)> = self.placements.iter().map(|(&id, c)| (id, c)).collect();
        by.sort_by(|a, b| b.1.pixels.cmp(&a.1.pixels).then(a.0.cmp(&b.0)));
        by
    }

    pub fn summary_line(&self) -> String {
        let mut line = format!(
            "{} placements, ground {}, sky {}",
            self.placements.len(),
            self.share(self.ground),
            self.share(self.sky)
        );
        if let Some((id, c)) = self.largest_first().first() {
            let _ = write!(line, ", most {id} {}", self.share(c.pixels));
        }
        line
    }
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

#[cfg(test)]
mod tests {
    use world::sight::frame::Placed;

    use super::*;

    fn sight_rgba(index: u32) -> [u8; 4] {
        let byte = |code: u32| (code * 4 + 2) as u8;
        [
            byte(index & 63),
            byte((index >> 6) & 63),
            byte(index >> 12),
            255,
        ]
    }

    #[test]
    fn a_frame_is_tallied_by_what_each_pixel_shows() {
        let farm = Placed {
            unique_id: 7,
            building: true,
            file: Arc::from("world/wmo/farm.wmo"),
        };
        let tree = Placed {
            unique_id: 3,
            building: false,
            file: Arc::from("world/tree.m2"),
        };
        let index = SightIndex::of(vec![tree, farm]);
        let frame: Vec<u8> = [0, 1, 2, 3, 3, 1, 3, 9000]
            .into_iter()
            .flat_map(sight_rgba)
            .collect();
        let coverage = Coverage::count(&frame, 4, &index);
        assert_eq!(
            (
                coverage.pixels,
                coverage.sky,
                coverage.ground,
                coverage.unnamed
            ),
            (8, 1, 2, 1)
        );
        let farm = &coverage.placements[&7];
        assert_eq!(
            (farm.pixels, farm.min, farm.max),
            (3, UVec2::new(0, 0), UVec2::new(3, 1))
        );
        assert_eq!(coverage.placements[&3].pixels, 1);
        let text = coverage.listing("--eye 1,2,3 --look 4,5,6", UVec2::new(4, 2), &[5]);
        let rows: Vec<&str> = text.lines().skip_while(|l| !l.starts_with("id ")).collect();
        assert_eq!(
            rows,
            [
                "id share pixels box colour kind model",
                "7 37.50% 3 0,0-3,1 #0e0202 building world/wmo/farm.wmo",
                "3 12.50% 1 2,0-2,0 #0a0202 doodad world/tree.m2",
            ]
        );
        assert!(text.contains("\nleft-out 5\n") && text.contains("\nunnamed 12.50%\n"));
    }
}
