//! Writing pictures: a drawn frame averaged down, and every file whole or not at all.

use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{RgbImage, RgbaImage};
use mpq::Chain;

/// A ground swatch is this many pixels square: the texture twice across and twice down, as the
/// ground repeats it, every 4⅙ yards.
pub(crate) const SWATCH: u32 = 240;

/// Writes `path` through `write`, which is handed a file beside it; the file takes its name only
/// once it is whole, so a run stopped halfway leaves no half-written file behind.
pub fn write_atomically(
    path: &Path,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    write(&part)?;
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Saves an RGBA frame `big` pixels square as a PNG `big / factor` pixels square, each pixel the
/// rounded mean of the `factor`×`factor` it covers.
pub fn save_averaged(rgba: &[u8], big: u32, factor: u32, path: &Path) -> Result<(), String> {
    if rgba.len() != (big as usize).pow(2) * 4 || factor == 0 || !big.is_multiple_of(factor) {
        return Err(format!(
            "a frame of {} bytes is not {big} pixels square in {factor}s",
            rgba.len()
        ));
    }
    let side = big / factor;
    let n = factor as usize;
    let d = factor * factor;
    let mut img = RgbImage::new(side, side);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let mut sum = [0u32; 3];
        for dy in 0..n {
            let row = (y as usize * n + dy) * big as usize;
            for dx in 0..n {
                let i = (row + x as usize * n + dx) * 4;
                for (s, v) in sum.iter_mut().zip(&rgba[i..i + 3]) {
                    *s += u32::from(*v);
                }
            }
        }
        px.0 = sum.map(|s| ((s + d / 2) / d) as u8);
    }
    write_atomically(path, |part| save_png(&img, part))
}

pub(crate) fn save_png(img: &RgbImage, path: &Path) -> Result<(), String> {
    img.save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Writes the ground texture at `path` as a swatch, its colour without the alpha the client reads
/// as shine.
pub(crate) fn swatch(chain: &Chain, path: &str, out: &Path) -> Result<(), String> {
    let bytes = chain.read(path).map_err(|e| format!("{path}: {e}"))?;
    let blp = blp::decode(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let top = blp
        .mips
        .first()
        .ok_or_else(|| format!("{path}: no image"))?;
    let rgba = RgbaImage::from_raw(top.width, top.height, top.rgba.clone())
        .ok_or_else(|| format!("{path}: a short image"))?;
    let tile = image::imageops::resize(&rgba, SWATCH / 2, SWATCH / 2, FilterType::Triangle);
    let mut img = RgbImage::new(SWATCH, SWATCH);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let [r, g, b, _] = tile.get_pixel(x % (SWATCH / 2), y % (SWATCH / 2)).0;
        px.0 = [r, g, b];
    }
    write_atomically(out, |part| save_png(&img, part))
}
