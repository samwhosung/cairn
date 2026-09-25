//! A zone drawn from above, from its terrain tiles: each pixel is the blend of the terrain
//! textures' average colours under the client's layer weights, lit by hill shading from the height
//! grid and tinted where water covers it. Doodads are dots by kind, buildings are squares, and the
//! pixels outside the zone are dimmed so its border reads.

mod areas;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::hash::BuildHasher;

use image::{Rgb, RgbImage};
use mpq::Chain;
use rayon::prelude::*;
use terrain::{ALPHA_MAP_SIZE, CHUNK_SIZE, ChunkMesh, TILE_SIZE, TileMesh, adt_to_tile_mesh};

pub use areas::{Area, Areas};

const MAX_SIDE: usize = 8192;
const AM: usize = ALPHA_MAP_SIZE as usize;
const ROW_STRIDE: usize = 17;
const LIGHT_FROM_NORTH_WEST: [f32; 3] = [-1.0, -1.0, 1.4];
const RING: [u8; 3] = [255, 0, 255];
const RING_INNER_SQ: i64 = 8 * 8;
const RING_OUTER: i64 = 11;

fn weights(alpha: Option<&[u8]>, i: usize, n: usize) -> [f32; 4] {
    let Some(a) = alpha else {
        return [1.0, 0.0, 0.0, 0.0];
    };
    let at = |ch: usize, layer: usize| {
        if n > layer {
            f32::from(a[i * 4 + ch]) / 255.0
        } else {
            0.0
        }
    };
    let (a1, a2, a3) = (at(0, 1), at(1, 2), at(2, 3));
    [
        (1.0 - a1) * (1.0 - a2) * (1.0 - a3),
        a1 * (1.0 - a2) * (1.0 - a3),
        a2 * (1.0 - a3),
        a3,
    ]
}

fn height(c: &ChunkMesh, fr: f32, fk: f32) -> f32 {
    let (r, k) = ((fr * 8.0).min(7.999), (fk * 8.0).min(7.999));
    let (r0, k0) = (r as usize, k as usize);
    let (tr, tk) = (r - r0 as f32, k - k0 as f32);
    let h = |r: usize, k: usize| c.positions[r * ROW_STRIDE + k][2];
    let top = h(r0, k0) * (1.0 - tk) + h(r0, k0 + 1) * tk;
    let bot = h(r0 + 1, k0) * (1.0 - tk) + h(r0 + 1, k0 + 1) * tk;
    top * (1.0 - tr) + bot * tr
}

fn surface(c: &ChunkMesh) -> Option<f32> {
    c.liquids
        .iter()
        .flat_map(|l| l.positions.iter().map(|p| p[2]))
        .reduce(f32::max)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Tree,
    Shrub,
    Rock,
    Fence,
    Prop,
}

fn kind(model: &str) -> Kind {
    let b = model
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
        .replace("dustwallow", "");
    if ["tree", "canopy", "palm", "trunk", "log"]
        .iter()
        .any(|k| b.contains(k))
    {
        Kind::Tree
    } else if [
        "bush", "shrub", "plant", "fern", "flower", "grass", "weed", "reed", "vine", "root",
        "mushroom", "cactus",
    ]
    .iter()
    .any(|k| b.contains(k))
    {
        Kind::Shrub
    } else if ["rock", "stone", "boulder", "cliff", "pebble"]
        .iter()
        .any(|k| b.contains(k))
    {
        Kind::Rock
    } else if ["fence", "post", "wall", "rail", "gate"]
        .iter()
        .any(|k| b.contains(k))
    {
        Kind::Fence
    } else {
        Kind::Prop
    }
}

/// The doodads a map drew, by kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Doodads {
    pub trees: usize,
    pub shrubs: usize,
    pub rocks: usize,
    pub fences: usize,
    pub props: usize,
}

#[derive(Clone, Copy)]
struct Ground {
    rgb: [f32; 3],
    z: f32,
    water_depth: Option<f32>,
    inside: bool,
}

const NO_GROUND: Ground = Ground {
    rgb: [0.0; 3],
    z: 0.0,
    water_depth: None,
    inside: false,
};

/// The tiles a zone covers on a map: the bounding box of every tile holding one of its chunks.
pub struct Frame {
    pub x0: u32,
    pub x1: u32,
    pub y0: u32,
    pub y1: u32,
}

impl Frame {
    /// The world X and Y of the picture's top-left corner. North is up and west is left.
    pub fn corner(&self) -> [f32; 2] {
        origin(self.x0, self.y0)
    }

    /// Where world `(x, y)` falls on the picture drawn at `ypp` yards a pixel: its column and
    /// row, in pixels from the top-left corner, unrounded.
    pub fn pixel(&self, ypp: f32, [x, y]: [f32; 2]) -> [f32; 2] {
        let [wx_max, wy_max] = self.corner();
        [(wy_max - y) / ypp, (wx_max - x) / ypp]
    }
}

fn origin(tx: u32, ty: u32) -> [f32; 2] {
    [
        32.0 * TILE_SIZE - ty as f32 * TILE_SIZE,
        32.0 * TILE_SIZE - tx as f32 * TILE_SIZE,
    ]
}

/// The frame of zone `zone_id` among `loaded`; an error when none of them holds the zone.
pub fn frame(
    loaded: &[((u32, u32), TileMesh)],
    areas: &Areas,
    zone_id: u32,
) -> Result<Frame, String> {
    let zone_of = |a: u32| areas.top_zone(a).unwrap_or(a);
    let hit: Vec<(u32, u32)> = loaded
        .iter()
        .filter(|(_, t)| t.chunks.iter().any(|c| zone_of(c.area_id) == zone_id))
        .map(|(k, _)| *k)
        .collect();
    if hit.is_empty() {
        return Err(format!("area {zone_id} has no terrain on this map"));
    }
    Ok(Frame {
        x0: hit.iter().map(|t| t.0).min().unwrap_or(0),
        x1: hit.iter().map(|t| t.0).max().unwrap_or(0),
        y0: hit.iter().map(|t| t.1).min().unwrap_or(0),
        y1: hit.iter().map(|t| t.1).max().unwrap_or(0),
    })
}

/// Every tile of the map under `World\Maps\<directory>` in the chain, meshed, in rows from the
/// north-west; a tile that fails to read or mesh is left out.
pub fn load_map(chain: &Chain, directory: &str) -> Vec<((u32, u32), TileMesh)> {
    let adt = |x: u32, y: u32| format!("World\\Maps\\{directory}\\{directory}_{x}_{y}.adt");
    let mut coords = Vec::new();
    for y in 0..64 {
        for x in 0..64 {
            if chain.contains(&adt(x, y)) {
                coords.push((x, y));
            }
        }
    }
    coords
        .par_iter()
        .filter_map(|&(x, y)| {
            let b = chain.read(&adt(x, y)).ok()?;
            Some(((x, y), adt_to_tile_mesh(&b).ok()?))
        })
        .collect()
}

/// The average colour of each texture the tiles' chunks name, from its largest level.
pub fn texture_colors(
    chain: &Chain,
    loaded: &[((u32, u32), TileMesh)],
) -> HashMap<String, [f32; 3]> {
    let mut tex_names: Vec<String> = loaded
        .iter()
        .flat_map(|(_, t)| {
            t.chunks
                .iter()
                .flat_map(|c| c.layer_textures.iter().cloned())
        })
        .collect();
    tex_names.sort();
    tex_names.dedup();
    tex_names
        .par_iter()
        .filter_map(|n| {
            let b = chain.read(n).ok()?;
            let img = blp::decode(&b).ok()?;
            let m = &img.mips[0];
            let mut s = [0f64; 3];
            for px in m.rgba.as_chunks::<4>().0 {
                for (sum, &v) in s.iter_mut().zip(px) {
                    *sum += f64::from(v);
                }
            }
            let n_px = (m.rgba.len() / 4).max(1) as f64;
            Some((
                n.clone(),
                [
                    (s[0] / n_px / 255.0) as f32,
                    (s[1] / n_px / 255.0) as f32,
                    (s[2] / n_px / 255.0) as f32,
                ],
            ))
        })
        .collect()
}

/// Draws the frame at `ypp` yards a pixel.
pub fn render<S: BuildHasher + Sync>(
    loaded: &[((u32, u32), TileMesh)],
    colors: &HashMap<String, [f32; 3], S>,
    areas: &Areas,
    zone_id: u32,
    f: &Frame,
    ypp: f32,
) -> Result<(RgbImage, Doodads), String> {
    let ppt = (TILE_SIZE / ypp).round() as usize;
    let side = |a: u32, b: u32| (b.saturating_sub(a) as usize + 1).saturating_mul(ppt);
    let (w, h) = (side(f.x0, f.x1), side(f.y0, f.y1));
    if !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
        return Err(format!(
            "the map would be {w}×{h} px, and a side must be 1 to {MAX_SIDE}"
        ));
    }
    let ground = ground(loaded, colors, areas, zone_id, f, ypp, (w, h));
    let mut img = light(&ground, (w, h), ypp);
    let drawn = marks(&mut img, loaded, f, ypp);
    Ok((img, drawn))
}

fn ground<S: BuildHasher + Sync>(
    loaded: &[((u32, u32), TileMesh)],
    colors: &HashMap<String, [f32; 3], S>,
    areas: &Areas,
    zone_id: u32,
    f: &Frame,
    ypp: f32,
    (w, h): (usize, usize),
) -> Vec<Ground> {
    let zone_of = |a: u32| areas.top_zone(a).unwrap_or(a);
    let [wx_max, wy_max] = f.corner();
    let mut index: HashMap<(i64, i64), &ChunkMesh> = HashMap::new();
    for ((tx, ty), tm) in loaded {
        let [ox, oy] = origin(*tx, *ty);
        for c in &tm.chunks {
            let r = ((ox - c.positions[0][0]) / CHUNK_SIZE).round() as i64;
            let k = ((oy - c.positions[0][1]) / CHUNK_SIZE).round() as i64;
            index.insert((i64::from(*tx) * 16 + k, i64::from(*ty) * 16 + r), c);
        }
    }
    let rows: Vec<Vec<Ground>> = (0..h)
        .into_par_iter()
        .map(|r| {
            (0..w)
                .map(|k| {
                    let wx = wx_max - (r as f32 + 0.5) * ypp;
                    let wy = wy_max - (k as f32 + 0.5) * ypp;
                    let (gxf, gyf) = (
                        (32.0 * TILE_SIZE - wy) / CHUNK_SIZE,
                        (32.0 * TILE_SIZE - wx) / CHUNK_SIZE,
                    );
                    let Some(c) = index.get(&(gxf.floor() as i64, gyf.floor() as i64)) else {
                        return NO_GROUND;
                    };
                    let (fr, fk) = (gyf - gyf.floor(), gxf - gxf.floor());
                    let n = c.layer_textures.len().min(4);
                    let ti = ((fr * 64.0) as usize).min(63) * AM + ((fk * 64.0) as usize).min(63);
                    let wt = weights(c.alpha_map.as_deref(), ti, n);
                    let mut rgb = if n == 0 { [0.4; 3] } else { [0.0; 3] };
                    for (l, t) in c.layer_textures.iter().take(n).enumerate() {
                        let lc = colors.get(t).copied().unwrap_or([0.5, 0.5, 0.5]);
                        for (ch, v) in rgb.iter_mut().zip(lc) {
                            *ch += wt[l] * v;
                        }
                    }
                    let z = height(c, fr, fk);
                    Ground {
                        rgb,
                        z,
                        water_depth: surface(c).filter(|s| z < *s).map(|s| s - z),
                        inside: zone_of(c.area_id) == zone_id,
                    }
                })
                .collect()
        })
        .collect();
    rows.concat()
}

fn light(ground: &[Ground], (w, h): (usize, usize), ypp: f32) -> RgbImage {
    let light = {
        let v = LIGHT_FROM_NORTH_WEST;
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / n, v[1] / n, v[2] / n]
    };
    let z = |r: usize, k: usize| ground[r * w + k].z;
    let mut img = RgbImage::new(w as u32, h as u32);
    for r in 0..h {
        for k in 0..w {
            let px = ground[r * w + k];
            let (zl, zr) = (z(r, k.saturating_sub(1)), z(r, (k + 1).min(w - 1)));
            let (zu, zd) = (z(r.saturating_sub(1), k), z((r + 1).min(h - 1), k));
            let (dx, dy) = ((zr - zl) / (2.0 * ypp), (zd - zu) / (2.0 * ypp));
            let nl = (dx * dx + dy * dy + 1.0).sqrt();
            let ndotl = ((-dx * light[0] - dy * light[1] + light[2]) / nl).max(0.0);
            let shade = 0.35 + 0.8 * ndotl;
            let mut col = px.rgb.map(|v| v * shade);
            if let Some(depth) = px.water_depth {
                let t = (depth / 12.0).clamp(0.25, 0.85);
                col = [
                    col[0] * (1.0 - t) + 0.10 * t,
                    col[1] * (1.0 - t) + 0.22 * t,
                    col[2] * (1.0 - t) + 0.35 * t,
                ];
            }
            if !px.inside {
                let g = (col[0] + col[1] + col[2]) / 3.0;
                col = [g * 0.45, g * 0.45, g * 0.5];
            }
            img.put_pixel(
                k as u32,
                r as u32,
                Rgb(col.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8)),
            );
        }
    }
    img
}

fn marks(img: &mut RgbImage, loaded: &[((u32, u32), TileMesh)], f: &Frame, ypp: f32) -> Doodads {
    let mut drawn = Doodads::default();
    for ((tx, ty), tm) in loaded {
        if *tx < f.x0 || *tx > f.x1 || *ty < f.y0 || *ty > f.y1 {
            continue;
        }
        for d in &tm.doodads {
            let [k, r] = f
                .pixel(ypp, [d.position[0], d.position[1]])
                .map(|v| v as i64);
            let (col, rad, count) = match kind(&d.model) {
                Kind::Tree => (
                    [20u8, 70, 25],
                    (2.5 * d.scale / ypp).max(1.0),
                    &mut drawn.trees,
                ),
                Kind::Shrub => ([90, 150, 60], (1.0 / ypp).max(0.5), &mut drawn.shrubs),
                Kind::Rock => (
                    [150, 150, 150],
                    (1.5 * d.scale / ypp).max(0.7),
                    &mut drawn.rocks,
                ),
                Kind::Fence => ([110, 70, 30], 0.7, &mut drawn.fences),
                Kind::Prop => ([230, 170, 40], 0.8, &mut drawn.props),
            };
            *count += 1;
            dot(img, k, r, rad, col);
        }
        for m in &tm.wmos {
            let [k, r] = f
                .pixel(ypp, [m.position[0], m.position[1]])
                .map(|v| v as i64);
            for dr in -3..=3i64 {
                for dc in -3..=3i64 {
                    if dr.abs() == 3 || dc.abs() == 3 {
                        put(
                            img,
                            k.saturating_add(dc),
                            r.saturating_add(dr),
                            [200, 30, 30],
                        );
                    }
                }
            }
        }
    }
    drawn
}

/// Rings world `(x, y)` on a picture of `f` drawn at `ypp` yards a pixel. False when the point is
/// off the picture.
pub fn mark(img: &mut RgbImage, f: &Frame, ypp: f32, point: [f32; 2]) -> bool {
    let [col, row] = f.pixel(ypp, point);
    let on = (0.0..img.width() as f32).contains(&col) && (0.0..img.height() as f32).contains(&row);
    if on {
        let (k, r) = (col as i64, row as i64);
        for dr in -RING_OUTER..=RING_OUTER {
            for dc in -RING_OUTER..=RING_OUTER {
                let d = dr * dr + dc * dc;
                if (RING_INNER_SQ..=RING_OUTER * RING_OUTER).contains(&d) {
                    put(img, k + dc, r + dr, RING);
                }
            }
        }
    }
    on
}

fn put(img: &mut RgbImage, x: i64, y: i64, c: [u8; 3]) {
    if (0..i64::from(img.width())).contains(&x) && (0..i64::from(img.height())).contains(&y) {
        img.put_pixel(x as u32, y as u32, Rgb(c));
    }
}

fn dot(img: &mut RgbImage, x: i64, y: i64, rad: f32, c: [u8; 3]) {
    let r = rad.ceil() as i64;
    for dy in -r..=r {
        for dx in -r..=r {
            if (dx * dx + dy * dy) as f32 <= rad * rad + 0.25 {
                put(img, x.saturating_add(dx), y.saturating_add(dy), c);
            }
        }
    }
}
