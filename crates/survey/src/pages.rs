//! Pages of pictures: twenty to a page, each numbered and named, so an agent can choose by looking
//! and then read the page's list to learn which file a number is.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{Rgb, RgbImage};

use crate::font;
use crate::picture::{save_png, write_atomically};

pub(crate) const COLUMNS: u32 = 5;
pub(crate) const ROWS: u32 = 4;
pub(crate) const PER_PAGE: usize = (COLUMNS * ROWS) as usize;
pub(crate) const CELL: u32 = 240;
const LABEL: u32 = 26;
const GUTTER: u32 = 6;
const TITLE: u32 = 24;
const PAGE: [u8; 3] = [38, 40, 44];
const INK: [u8; 3] = [232, 232, 232];
const DIM: [u8; 3] = [170, 176, 184];
const MISSING: [u8; 3] = [90, 30, 30];

/// One cell of a page: the picture, and two short lines under it.
pub(crate) struct Cell {
    pub(crate) picture: PathBuf,
    pub(crate) name: String,
    pub(crate) facts: String,
}

/// Draws `cells` (at most [`PER_PAGE`]) numbered from `first`, under `title`.
pub(crate) fn draw(title: &str, first: usize, cells: &[Cell], out: &Path) -> Result<(), String> {
    let width = GUTTER + COLUMNS * (CELL + GUTTER);
    let rows = (cells.len() as u32).div_ceil(COLUMNS).max(1);
    let height = TITLE + rows * (CELL + LABEL + GUTTER);
    let mut page = RgbImage::from_pixel(width, height, Rgb(PAGE));
    font::draw(&mut page, (GUTTER, 8), title, 1, 190, INK);
    for (i, cell) in cells.iter().enumerate() {
        let (col, row) = (i as u32 % COLUMNS, i as u32 / COLUMNS);
        let (x, y) = (
            GUTTER + col * (CELL + GUTTER),
            TITLE + row * (CELL + LABEL + GUTTER),
        );
        match thumbnail(&cell.picture) {
            Some(thumb) => image::imageops::replace(&mut page, &thumb, x.into(), y.into()),
            None => {
                for yy in y..y + CELL {
                    for xx in x..x + CELL {
                        page.put_pixel(xx, yy, Rgb(MISSING));
                    }
                }
            }
        }
        let number = (first + i).to_string();
        font::draw(&mut page, (x, y + CELL + 4), &number, 2, 4, INK);
        let text_x = x + (number.len() as u32 * font::ADVANCE + 1) * 2;
        let room = ((x + CELL).saturating_sub(text_x) / font::ADVANCE) as usize;
        font::draw(&mut page, (text_x, y + CELL + 4), &cell.name, 1, room, INK);
        font::draw(
            &mut page,
            (text_x, y + CELL + 14),
            &cell.facts,
            1,
            room,
            DIM,
        );
    }
    write_atomically(out, |part| save_png(&page, part))
}

/// The picture at `path` fitted to a cell: halved by averaging when it is twice the cell, as the
/// catalog's own pictures are, else resampled.
fn thumbnail(path: &Path) -> Option<RgbImage> {
    let img = image::open(path).ok()?.to_rgb8();
    if img.dimensions() == (CELL, CELL) {
        return Some(img);
    }
    if img.dimensions() == (2 * CELL, 2 * CELL) {
        let mut out = RgbImage::new(CELL, CELL);
        for (x, y, px) in out.enumerate_pixels_mut() {
            let mut sum = [0u32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = img.get_pixel(2 * x + dx, 2 * y + dy).0;
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += u32::from(v);
                }
            }
            px.0 = sum.map(|s| ((s + 2) / 4) as u8);
        }
        return Some(out);
    }
    Some(image::imageops::resize(
        &img,
        CELL,
        CELL,
        FilterType::Triangle,
    ))
}
