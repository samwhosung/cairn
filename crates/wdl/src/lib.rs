//! Reads World of Warcraft 1.12.1 WDL maps: the coarse heights the horizon is drawn from.

mod error;
mod mesh;

use wowfile::ByteExt;

pub use error::Error;
pub use mesh::WdlTileMesh;

const TILES_PER_MAP: usize = 64;
const OUTER_EDGE: usize = 17;
const INNER_EDGE: usize = 16;
const OUTER_N: usize = OUTER_EDGE * OUTER_EDGE;
const INNER_N: usize = INNER_EDGE * INNER_EDGE;
const MARE_BYTES: usize = (OUTER_N + INNER_N) * 2;
const MAOF_BYTES: usize = TILES_PER_MAP * TILES_PER_MAP * 4;
const VERSION: u32 = 18;

/// A map's WDL: one height per ADT chunk corner and chunk centre, for every tile the map has.
pub struct WdlFile {
    tiles: Vec<Option<Heights>>,
}

/// One tile's `MARE` heights in yards, row-major.
struct Heights {
    corners: [i16; OUTER_N],
    centres: [i16; INNER_N],
}

fn maof_index(tile_x: usize, tile_y: usize) -> usize {
    tile_y * TILES_PER_MAP + tile_x
}

impl WdlFile {
    /// Parses a WDL from its first chunk, which must be `MVER` at version 18.
    pub fn parse(b: &[u8]) -> Result<Self, Error> {
        let (magic, size) = header(b, 0)?;
        if &magic != b"REVM" {
            return Err(Error::NotWdl);
        }
        let version = b.u32_at(8).ok_or(Error::Truncated("MVER"))?;
        if version != VERSION {
            return Err(Error::Version(version));
        }
        let mut pos = 8 + size as usize;
        let offsets: Vec<u32> = loop {
            let (magic, size) = header(b, pos)?;
            let body = pos + 8;
            if &magic == b"FOAM" {
                if size as usize != MAOF_BYTES {
                    return Err(Error::MaofSize(size));
                }
                let raw = b
                    .bytes_at(body, MAOF_BYTES)
                    .ok_or(Error::Truncated("MAOF"))?;
                break raw
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| u32::from_le_bytes(*c))
                    .collect();
            }
            pos = body + size as usize;
        };
        let mut tiles = Vec::with_capacity(offsets.len());
        for (index, &offset) in offsets.iter().enumerate() {
            if offset == 0 {
                tiles.push(None);
                continue;
            }
            let at = offset as usize;
            let (magic, size) = header(b, at)?;
            if &magic != b"ERAM" || (size as usize) < MARE_BYTES {
                return Err(Error::BadMare { index });
            }
            let body = b
                .bytes_at(at + 8, MARE_BYTES)
                .ok_or(Error::Truncated("MARE"))?;
            let height = |i: usize| i16::from_le_bytes([body[i * 2], body[i * 2 + 1]]);
            tiles.push(Some(Heights {
                corners: std::array::from_fn(height),
                centres: std::array::from_fn(|i| height(OUTER_N + i)),
            }));
        }
        Ok(Self { tiles })
    }

    /// How many tiles the map has.
    pub fn present_count(&self) -> usize {
        self.tiles.iter().filter(|t| t.is_some()).count()
    }

    /// Whether tile `(tile_x, tile_y)`, as in `Map_<x>_<y>.adt`, has heights.
    pub fn is_present(&self, tile_x: u32, tile_y: u32) -> bool {
        self.heights(tile_x, tile_y).is_some()
    }

    /// Every tile with heights within `radius` tiles of world `(x, y)`, the tile under it
    /// included.
    pub fn tiles_around(&self, world_x: f32, world_y: f32, radius: u32) -> Vec<(u32, u32)> {
        let (cx, cy) = wdt::world_to_tile(world_x, world_y);
        let r = radius as i32;
        let mut out = Vec::new();
        for dy in -r..=r {
            for dx in -r..=r {
                let (tx, ty) = (cx as i32 + dx, cy as i32 + dy);
                if tx >= 0 && ty >= 0 && self.is_present(tx as u32, ty as u32) {
                    out.push((tx as u32, ty as u32));
                }
            }
        }
        out
    }

    fn heights(&self, tile_x: u32, tile_y: u32) -> Option<&Heights> {
        let (x, y) = (tile_x as usize, tile_y as usize);
        if x >= TILES_PER_MAP || y >= TILES_PER_MAP {
            return None;
        }
        self.tiles[maof_index(x, y)].as_ref()
    }
}

fn header(b: &[u8], at: usize) -> Result<([u8; 4], u32), Error> {
    let magic = b.bytes_at(at, 4).ok_or(Error::Truncated("chunk header"))?;
    let size = b.u32_at(at + 4).ok_or(Error::Truncated("chunk header"))?;
    Ok(([magic[0], magic[1], magic[2], magic[3]], size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = magic.to_vec();
        v.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        v.extend_from_slice(payload);
        v
    }

    fn one_tile_file(tile_x: usize, tile_y: usize) -> Vec<u8> {
        let mut file = chunk(*b"REVM", &VERSION.to_le_bytes());
        let mare_at = file.len() + 8 + MAOF_BYTES;
        let mut maof = vec![0u8; MAOF_BYTES];
        let slot = maof_index(tile_x, tile_y) * 4;
        maof[slot..slot + 4].copy_from_slice(&(mare_at as u32).to_le_bytes());
        file.extend(chunk(*b"FOAM", &maof));
        let heights: Vec<u8> = (0..(OUTER_N + INNER_N) as i16)
            .flat_map(i16::to_le_bytes)
            .collect();
        file.extend(chunk(*b"ERAM", &heights));
        file
    }

    #[test]
    fn a_tile_is_found_where_maof_points() {
        let wdl = WdlFile::parse(&one_tile_file(34, 48)).expect("parses");
        assert_eq!(wdl.present_count(), 1);
        assert!(wdl.is_present(34, 48));
        assert!(!wdl.is_present(48, 34));
        assert!(!wdl.is_present(64, 0));
        let heights = wdl.heights(34, 48).expect("present");
        assert_eq!(
            (heights.corners[1], heights.centres[0]),
            (1, OUTER_N as i16)
        );
    }

    #[test]
    fn malformed_files_are_refused() {
        assert!(matches!(WdlFile::parse(b"REVM"), Err(Error::Truncated(_))));
        let mut other = one_tile_file(0, 0);
        other[8] = 17;
        assert!(matches!(WdlFile::parse(&other), Err(Error::Version(17))));
        let mut bad_mare = one_tile_file(0, 0);
        let at = bad_mare.len() - MARE_BYTES - 8;
        bad_mare[at] = b'X';
        assert!(matches!(
            WdlFile::parse(&bad_mare),
            Err(Error::BadMare { index: 0 })
        ));
    }
}
