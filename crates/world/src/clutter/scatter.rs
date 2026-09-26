use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::Arc;

use mpq::Chain;
use terrain::{ChunkMesh, VERTICES, cell_vertices, is_hole};

use crate::dbc_table::{read_table, str_at, u32_at};

/// A doodad names `ElwGra01.mdl`; the file is that stem's `.m2` here, left out of the listfiles.
const DETAIL_DIR: &str = "World\\NoDXT\\Detail\\";
const NO_DOODAD: u32 = u32::MAX;
const DEFAULT_DENSITY: u32 = 8;

/// The permutation of 0..=255 the client's ground-clutter randomizer mixes.
#[rustfmt::skip]
const NOISE: [u8; 256] = [
    0x8e, 0x14, 0x27, 0x99, 0xfd, 0xaa, 0xc7, 0x08, 0xd5, 0xe6, 0x3e, 0x1f, 0xf6, 0xbb, 0x55, 0xda,
    0x75, 0xa0, 0x4a, 0x6a, 0xe8, 0xbd, 0x97, 0xff, 0xde, 0x9b, 0xbc, 0x9f, 0x81, 0x8a, 0xa1, 0x46,
    0x6e, 0x0b, 0xe3, 0x63, 0x76, 0x7a, 0x6c, 0x5d, 0x88, 0xd3, 0x69, 0xca, 0xc3, 0x47, 0xb9, 0x25,
    0x83, 0xab, 0xa2, 0x3f, 0xa6, 0x41, 0x7c, 0xba, 0xe5, 0xac, 0x95, 0x01, 0x7e, 0xcf, 0x09, 0xc1,
    0xd9, 0x62, 0x70, 0x71, 0x8d, 0xdb, 0x05, 0x02, 0x24, 0x87, 0xef, 0x54, 0xc6, 0xd4, 0x37, 0x30,
    0xd0, 0x1b, 0xcb, 0x7b, 0xb8, 0xe4, 0xd8, 0xec, 0x49, 0xce, 0xad, 0xdc, 0x13, 0xa9, 0x94, 0xc4,
    0x8f, 0x39, 0xae, 0x0d, 0x18, 0x52, 0xdd, 0x0e, 0x78, 0xfa, 0xf5, 0x85, 0x58, 0xd2, 0xaf, 0x6d,
    0xa4, 0xb2, 0x53, 0x3b, 0x51, 0xa5, 0x50, 0xbe, 0xfc, 0x2d, 0xf4, 0x11, 0x48, 0x98, 0x16, 0xf1,
    0x86, 0xdf, 0x3d, 0x66, 0x5e, 0x44, 0x2e, 0x2f, 0x36, 0x07, 0x6b, 0x17, 0x8b, 0x29, 0x4c, 0xb6,
    0xe2, 0x89, 0x5f, 0xe7, 0xcd, 0xa7, 0x21, 0xe1, 0x4d, 0xc9, 0x65, 0xed, 0xfe, 0xee, 0x9c, 0x23,
    0x33, 0x7d, 0xb7, 0x04, 0x9e, 0x9a, 0x2a, 0x40, 0xb3, 0x10, 0x5b, 0xf3, 0x82, 0x77, 0x1c, 0x92,
    0x20, 0x4e, 0x1e, 0x57, 0x22, 0x72, 0x06, 0x8c, 0x67, 0x2c, 0x73, 0xfb, 0x59, 0xc2, 0x0a, 0xbf,
    0x79, 0x5c, 0xf9, 0x0c, 0x28, 0x1a, 0x12, 0x68, 0x74, 0x34, 0x19, 0x42, 0xb1, 0xc0, 0x84, 0xf8,
    0x38, 0xf0, 0x15, 0x9d, 0x60, 0xf2, 0x3a, 0x6f, 0xb4, 0x90, 0xeb, 0x91, 0x1d, 0x7f, 0x35, 0x61,
    0x5a, 0x32, 0x03, 0x56, 0xa3, 0xc5, 0x2b, 0x93, 0x80, 0x0f, 0x4b, 0x43, 0xf7, 0xa8, 0xe0, 0x3c,
    0x96, 0xd1, 0x64, 0x26, 0xd7, 0x45, 0xcc, 0x4f, 0xc8, 0xb0, 0xe9, 0xb5, 0x00, 0xd6, 0x31, 0xea,
];

struct GroundEffect {
    doodads: [Option<Arc<str>>; 4],
    density: u32,
}

pub(crate) struct Effects(HashMap<u32, GroundEffect>);

impl Effects {
    pub(crate) fn read(chain: &Chain) -> Result<Self, String> {
        let rs = read_table(chain, "DBFilesClient\\GroundEffectDoodad.dbc", 3, &[2])?;
        // A slot names a `GroundEffectDoodad` row by its second column, not by its id.
        let models: HashMap<u32, Arc<str>> = rs
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 1)?, detail_model(&str_at(&rs, r, 2))?)))
            .collect();
        let rs = read_table(chain, "DBFilesClient\\GroundEffectTexture.dbc", 7, &[])?;
        let mut effects = HashMap::new();
        for r in rs.records() {
            let Some(id) = u32_at(r, 0) else {
                continue;
            };
            let doodads: [Option<Arc<str>>; 4] = std::array::from_fn(|i| {
                let doodad = u32_at(r, 1 + i).unwrap_or(0);
                (doodad != NO_DOODAD)
                    .then(|| models.get(&doodad).cloned())
                    .flatten()
            });
            if doodads.iter().any(Option::is_some) {
                let density = u32_at(r, 5).filter(|&d| d != 0).unwrap_or(DEFAULT_DENSITY);
                effects.insert(id, GroundEffect { doodads, density });
            }
        }
        Ok(Self(effects))
    }
}

fn detail_model(name: &str) -> Option<Arc<str>> {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    (!name.is_empty()).then(|| format!("{DETAIL_DIR}{stem}.m2").into())
}

pub(crate) struct Tuft {
    pub(crate) model: Arc<str>,
    /// World coordinates: x north, y west, z up.
    pub(crate) position: [f32; 3],
    pub(crate) yaw: f32,
    pub(crate) scale: f32,
}

/// A chunk's column and row among all of its map's chunks.
pub(crate) fn map_chunk((tile_x, tile_y): (u32, u32), chunk: &ChunkMesh) -> (u32, u32) {
    (tile_x * 16 + chunk.index_x, tile_y * 16 + chunk.index_y)
}

/// The tufts the client places on `chunk` of `tile`.
#[allow(
    clippy::manual_midpoint,
    reason = "the client's own sum-then-halve rounding"
)]
pub(crate) fn scatter(
    chunk: &ChunkMesh,
    tile: (u32, u32),
    effects: &Effects,
    cell_draws: u32,
) -> Vec<Tuft> {
    if chunk.positions.len() < VERTICES {
        return Vec::new();
    }
    let (x, y) = map_chunk(tile, chunk);
    let mut rng = Randomizer::new((y << 16) | (x & 0xFFFF));
    let drawn: Vec<(u32, u32)> = (0..cell_draws)
        .map(|_| {
            let col = rng.next() & 7;
            let row = rng.next() & 7;
            (row, col)
        })
        .collect();
    let mut out = Vec::new();
    for (list_index, &(row, col)) in drawn.iter().enumerate() {
        let k = (row * 8 + col) as usize;
        if chunk.no_effect_doodad[k] || is_hole(chunk.holes, row, col) {
            continue;
        }
        let Some(effect) = chunk
            .layer_effect_ids
            .get(usize::from(chunk.pred_tex[k]))
            .and_then(|id| effects.0.get(id))
        else {
            continue;
        };
        let cell = cell_vertices(row, col).map(|i| chunk.positions[i as usize]);
        for n in 0..effect.density as usize {
            let (rx, ry) = (rng.signed_unit(), rng.signed_unit());
            let Some(model) = &effect.doodads[(n + list_index) & 3] else {
                continue;
            };
            let scale = rng.signed_unit() * 0.1 + 1.0;
            let yaw = rng.signed_unit() * PI;
            out.push(Tuft {
                model: model.clone(),
                position: fan_point((rx + 1.0) * 0.5, (ry + 1.0) * 0.5, cell),
                yaw,
                scale,
            });
        }
    }
    out
}

fn fan_point(east: f32, south: f32, [tl, tr, bl, br, ctr]: [[f32; 3]; 5]) -> [f32; 3] {
    let (u, v, w, a, b) = if east + south <= 1.0 {
        if south <= east {
            (2.0 * south, 1.0 - east - south, east - south, tl, tr)
        } else {
            (2.0 * east, south - east, 1.0 - east - south, bl, tl)
        }
    } else if south >= east {
        (
            2.0 * (1.0 - south),
            east + south - 1.0,
            south - east,
            br,
            bl,
        )
    } else {
        (2.0 * (1.0 - east), east - south, east + south - 1.0, tr, br)
    };
    std::array::from_fn(|i| u * ctr[i] + v * a[i] + w * b[i])
}

/// The client's ground-clutter randomizer.
struct Randomizer {
    source: u32,
    seed: u32,
}

impl Randomizer {
    fn new(source: u32) -> Self {
        let seed = ((source % 0x2F) << 26)
            | ((source % 0x35) << 18)
            | ((source % 0x3B) << 10)
            | (4 * (source % 0x3D));
        Self { source, seed }
    }

    fn next(&mut self) -> u32 {
        // The client wraps a lane below zero by the lane's own constant, not 256, so no lane
        // passes 251 and its four noise bytes stay in the table.
        let lane = |byte: u32, sub: u32, wrap: u32| {
            let b = byte & 0xFF;
            if b < sub { b + wrap - sub } else { b - sub }
        };
        let a = lane(self.seed, 0x1C, 0xF4);
        let b = lane(self.seed >> 8, 0x18, 0xEC);
        let c = lane(self.seed >> 16, 0x0C, 0xD4);
        let d = lane(self.seed >> 24, 0x04, 0xBC);
        self.seed = a | (b << 8) | (c << 16) | (d << 24);
        let noise = |i: u32| u32::from_le_bytes(std::array::from_fn(|k| NOISE[i as usize + k]));
        self.source = self.source.wrapping_add(
            noise(a) ^ noise(d).rotate_left(1) ^ noise(c).rotate_left(2) ^ noise(b).rotate_left(3),
        );
        self.source
    }

    fn signed_unit(&mut self) -> f32 {
        let u = self.next();
        let f = f32::from_bits((u & 0x007F_FFFF) | 0x3F80_0000);
        if (u as i32) < 0 { 2.0 - f } else { f - 2.0 }
    }
}

#[cfg(test)]
mod tests;
