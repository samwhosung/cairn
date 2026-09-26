use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{Rgb, RgbImage};

use crate::font;
use crate::picture::{save_png, write_atomically};

pub(crate) const COLUMNS: u32 = 5;
pub(crate) const ROWS: u32 = 4;
pub(crate) const PER_PAGE: usize = (COLUMNS * ROWS) as usize;
pub(crate) const CELL: u32 = 240;
/// A model's picture is this many pixels square: two cells of a page, which averages it down.
pub const PICTURE_SIDE: u32 = 2 * CELL;
const LABEL: u32 = 26;
const GUTTER: u32 = 6;
const TITLE: u32 = 24;
const PAGE: [u8; 3] = [38, 40, 44];
const INK: [u8; 3] = [232, 232, 232];
const DIM: [u8; 3] = [170, 176, 184];
const NUMBER_SCALE: u32 = 2;

pub(crate) struct Cell {
    pub(crate) picture: PathBuf,
    pub(crate) name: String,
    pub(crate) facts: String,
}

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
        let thumb = thumbnail(&cell.picture)
            .ok_or_else(|| format!("{}: no picture", cell.picture.display()))?;
        image::imageops::replace(&mut page, &thumb, x.into(), y.into());
        let number = (first + i).to_string();
        font::draw(&mut page, (x, y + CELL + 4), &number, NUMBER_SCALE, 4, INK);
        let text_x = x + (number.len() as u32 * font::ADVANCE + 1) * NUMBER_SCALE;
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

fn thumbnail(path: &Path) -> Option<RgbImage> {
    let img = image::open(path).ok()?.to_rgb8();
    if img.dimensions() == (CELL, CELL) {
        return Some(img);
    }
    if img.dimensions() == (PICTURE_SIDE, PICTURE_SIDE) {
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
