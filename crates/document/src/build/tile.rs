//! One ADT tile's bytes, laid out as every shipped tile is: `MVER MHDR MCIN MTEX MMDX MMID MWMO
//! MWID MDDF MODF`, then 256 `MCNK` in row order. `MHDR`'s offsets count from its payload,
//! `MCIN`'s from the file's start, and its sizes are whole records'. An `MCNK` is a 128-byte
//! header and then `MCVT MCNR MCLY MCRF MCSH MCAL MCLQ MCSE`, each offset counted from the chunk's
//! magic; `MCNR` has 13 bytes its size doesn't count, and `MCLQ` says 0 and is followed by its
//! blocks.

use crate::build::alpha::PACKED_BYTES;
use crate::build::liquid::RIVER_FLAG;
use crate::build::place::Tables;

const MCLY_HAS_ALPHA: u32 = 0x100;

/// The map's middle and a chunk's edge as the f32s the shipped corners were computed with.
const CENTRE: f32 = (32.0f64 * 1600.0 / 3.0) as f32;
const CHUNK: f32 = (1600.0f64 / 3.0 / 16.0) as f32;

/// The world corner `(x north, y west)` of chunk `(row, col)` of a tile, as the shipped tiles
/// store it.
pub fn chunk_corner(tile: (u32, u32), row: usize, col: usize) -> (f32, f32) {
    let (tx, ty) = (tile.0 as usize, tile.1 as usize);
    (
        CENTRE - (ty * 16 + row) as f32 * CHUNK,
        CENTRE - (tx * 16 + col) as f32 * CHUNK,
    )
}

pub struct Chunk {
    pub base: f32,
    pub mcvt: [f32; 145],
    pub normals: [[u8; 3]; 145],
    /// The base first.
    pub layers: Vec<Layer>,
    pub alpha: Vec<[u8; PACKED_BYTES]>,
    pub predominant: [u8; 16],
    pub water: Option<Vec<u8>>,
    pub area: u32,
    pub doodad_refs: Vec<u32>,
    pub wmo_refs: Vec<u32>,
}

pub struct Layer {
    pub mtex_index: u32,
    pub effect: u32,
}

fn record(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(8 + payload.len());
    v.extend_from_slice(&magic);
    v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

const SHADOW_BYTES: usize = 512;
const NORMAL_PAD: usize = 13;

fn chunk(tile: (u32, u32), e: usize, c: &Chunk) -> Vec<u8> {
    let (row, col) = (e / 16, e % 16);
    let heights: Vec<u8> = c.mcvt.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut normals = record(*b"RNCM", c.normals.as_flattened());
    normals.extend_from_slice(&[0; NORMAL_PAD]);
    let layers: Vec<u8> = c
        .layers
        .iter()
        .enumerate()
        .flat_map(|(i, l)| {
            let (flags, at) = if i == 0 {
                (0, 0)
            } else {
                (MCLY_HAS_ALPHA, PACKED_BYTES as u32 * (i as u32 - 1))
            };
            [l.mtex_index, flags, at, l.effect]
                .into_iter()
                .flat_map(u32::to_le_bytes)
        })
        .collect();
    let refs: Vec<u8> = c
        .doodad_refs
        .iter()
        .chain(&c.wmo_refs)
        .flat_map(|r| r.to_le_bytes())
        .collect();
    let mut water = record(*b"QLCM", &[]);
    water.extend_from_slice(c.water.as_deref().unwrap_or_default());
    let segments: [(usize, Vec<u8>); 8] = [
        (0x14, record(*b"TVCM", &heights)),
        (0x18, normals),
        (0x1C, record(*b"YLCM", &layers)),
        (0x20, record(*b"FRCM", &refs)),
        (0x2C, record(*b"HSCM", &[0; SHADOW_BYTES])),
        (0x24, record(*b"LACM", c.alpha.as_flattened())),
        (0x60, water),
        (0x58, record(*b"ESCM", &[])),
    ];
    let mut h = vec![0u8; 128];
    let mut at = 128usize;
    for (field, s) in &segments {
        put(&mut h, *field, (8 + at) as u32);
        at += s.len();
    }
    put(&mut h, 0x00, if c.water.is_some() { RIVER_FLAG } else { 0 });
    put(&mut h, 0x04, col as u32);
    put(&mut h, 0x08, row as u32);
    put(&mut h, 0x0C, c.layers.len() as u32);
    put(&mut h, 0x10, c.doodad_refs.len() as u32);
    put(&mut h, 0x28, segments[5].1.len() as u32);
    put(&mut h, 0x30, SHADOW_BYTES as u32);
    put(&mut h, 0x34, c.area);
    put(&mut h, 0x38, c.wmo_refs.len() as u32);
    h[0x40..0x50].copy_from_slice(&c.predominant);
    put(&mut h, 0x64, segments[6].1.len() as u32);
    let (x, y) = chunk_corner(tile, row, col);
    h[0x68..0x6C].copy_from_slice(&x.to_le_bytes());
    h[0x6C..0x70].copy_from_slice(&y.to_le_bytes());
    h[0x70..0x74].copy_from_slice(&c.base.to_le_bytes());
    for (_, s) in segments {
        h.extend_from_slice(&s);
    }
    record(*b"KNCM", &h)
}

pub fn tile(
    key: (u32, u32),
    textures: &[&str],
    doodads: &Tables,
    wmos: &Tables,
    chunks: &[Chunk],
) -> Vec<u8> {
    let mtex: Vec<u8> = textures.iter().flat_map(|t| t.bytes().chain([0])).collect();
    let mut out = record(*b"REVM", &18u32.to_le_bytes());
    let mhdr_at = out.len();
    out.extend(record(*b"RDHM", &[0; 64]));
    let mcin_at = out.len();
    out.extend(record(*b"NICM", &[0; 4096]));
    let mut slots = vec![mcin_at];
    for (magic, payload) in [
        (b"XETM", &mtex),
        (b"XDMM", &doodads.names),
        (b"DIMM", &doodads.starts),
        (b"OMWM", &wmos.names),
        (b"DIWM", &wmos.starts),
        (b"FDDM", &doodads.records),
        (b"FDOM", &wmos.records),
    ] {
        slots.push(out.len());
        out.extend(record(*magic, payload));
    }
    for (k, at) in slots.iter().enumerate() {
        put(&mut out, mhdr_at + 12 + k * 4, (at - (mhdr_at + 8)) as u32);
    }
    for (e, c) in chunks.iter().enumerate() {
        let at = out.len();
        out.extend(chunk(key, e, c));
        let entry = mcin_at + 8 + e * 16;
        put(&mut out, entry, at as u32);
        let size = (out.len() - at) as u32;
        put(&mut out, entry + 4, size);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_follow_the_shipped_rounding() {
        let t = (32, 48);
        assert_eq!(chunk_corner(t, 0, 0), (-8_533.334, 0.0));
        assert_eq!(chunk_corner(t, 0, 1), (-8_533.334, -33.333_984));
        assert_eq!(chunk_corner(t, 1, 1), (-8_566.666, -33.333_984));
        assert_eq!(chunk_corner(t, 15, 15), (-9_033.332, -500.0));
    }
}
