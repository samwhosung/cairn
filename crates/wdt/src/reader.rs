use std::io::{self, Read, Seek, SeekFrom};

use wowfile::ByteExt;

use crate::coords::TILES_PER_MAP;
use crate::error::Error;

const MAIN_ENTRY: usize = 8;
const MAIN_BYTES: usize = TILES_PER_MAP * TILES_PER_MAP * MAIN_ENTRY;
const MODF_ENTRY: u64 = 64;

/// One tile of the map table.
#[derive(Debug, Clone, Copy)]
pub struct TileInfo {
    pub x: usize,
    pub y: usize,
    pub has_adt: bool,
}

/// The one building that is the whole of a map with no terrain.
#[derive(Debug, Clone)]
pub struct GlobalWmo {
    /// The WMO root path.
    pub model: String,
    /// World coordinates (X north, Y west, Z up), as stored: unlike an ADT's placements, this
    /// one is not offset from the map corner.
    pub position: [f32; 3],
    /// Euler angles in degrees; Y is the heading.
    pub rotation: [f32; 3],
    /// The doodad set shown in addition to set 0.
    pub doodad_set: u16,
    /// The `WMOAreaTable` name set.
    pub name_set: u16,
}

/// A parsed WDT: which tiles have terrain, and the global building on a map without any.
#[derive(Debug)]
pub struct WdtFile {
    /// Indexed `y * 64 + x`, the order `MAIN` stores them in.
    has_adt: Vec<bool>,
    global_wmo: Option<GlobalWmo>,
}

impl WdtFile {
    /// The tile at `(x, y)`, or `None` off the 64×64 grid.
    pub fn get_tile(&self, x: usize, y: usize) -> Option<TileInfo> {
        if x >= TILES_PER_MAP || y >= TILES_PER_MAP {
            return None;
        }
        Some(TileInfo {
            x,
            y,
            has_adt: self.has_adt[y * TILES_PER_MAP + x],
        })
    }

    /// The building that is the whole map, when the map has no terrain.
    pub fn global_wmo(&self) -> Option<&GlobalWmo> {
        self.global_wmo.as_ref()
    }
}

/// Reads a [`WdtFile`] from a stream positioned at its first chunk.
pub struct WdtReader<R> {
    reader: R,
}

impl<R: Read + Seek> WdtReader<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }

    /// Reads chunks to the end of the stream. A map claims a global building only when `MPHD`
    /// flags it as terrainless and both its `MWMO` path and `MODF` placement are present.
    pub fn read(&mut self) -> Result<WdtFile, Error> {
        let start = self.reader.stream_position()?;
        let stream_end = self.reader.seek(SeekFrom::End(0))?;
        self.reader.seek(SeekFrom::Start(start))?;
        let mut has_adt = None;
        let mut terrainless = false;
        let mut wmo_path = None;
        let mut modf = None;
        while let Some((magic, size)) = self.chunk_header()? {
            match &magic {
                b"DHPM" if size >= 4 => {
                    let mphd = self.payload(size, stream_end)?;
                    terrainless = mphd.u32_at(0).is_some_and(|flags| flags & 0x1 != 0);
                }
                // Always a full table, whatever size the header gives.
                b"NIAM" => {
                    let mut main = vec![0u8; MAIN_BYTES];
                    self.reader.read_exact(&mut main)?;
                    has_adt = Some(
                        main.as_chunks::<MAIN_ENTRY>()
                            .0
                            .iter()
                            .map(|entry| entry[0] & 0x1 != 0)
                            .collect::<Vec<_>>(),
                    );
                }
                // A map with terrain still carries an empty `MWMO`.
                b"OMWM" if size > 0 => {
                    let mwmo = self.payload(size, stream_end)?;
                    let end = mwmo.iter().position(|&b| b == 0).unwrap_or(mwmo.len());
                    if end > 0 {
                        wmo_path = Some(String::from_utf8_lossy(&mwmo[..end]).into_owned());
                    }
                }
                b"FDOM" if size >= MODF_ENTRY => modf = Some(self.payload(size, stream_end)?),
                _ => {
                    self.reader.seek(SeekFrom::Current(size as i64))?;
                }
            }
        }
        let has_adt = has_adt.ok_or(Error::MissingMain)?;
        let global_wmo = match (terrainless, wmo_path, modf) {
            (true, Some(model), Some(e)) => Some(GlobalWmo {
                model,
                position: [f32_at(&e, 8), f32_at(&e, 12), f32_at(&e, 16)],
                rotation: [f32_at(&e, 20), f32_at(&e, 24), f32_at(&e, 28)],
                doodad_set: e.u16_at(58).unwrap_or(0),
                name_set: e.u16_at(60).unwrap_or(0),
            }),
            _ => None,
        };
        Ok(WdtFile {
            has_adt,
            global_wmo,
        })
    }

    /// The next chunk's magic (as stored, reversed) and size, or `None` at a clean end of stream.
    fn chunk_header(&mut self) -> Result<Option<([u8; 4], u64)>, Error> {
        let mut header = [0u8; 8];
        let mut filled = 0;
        while filled < header.len() {
            match self.reader.read(&mut header[filled..]) {
                Ok(0) if filled == 0 => return Ok(None),
                Ok(0) => return Err(Error::TruncatedHeader),
                Ok(n) => filled += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        let magic = [header[0], header[1], header[2], header[3]];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
        Ok(Some((magic, u64::from(size))))
    }

    fn payload(&mut self, size: u64, stream_end: u64) -> Result<Vec<u8>, Error> {
        let remaining = stream_end.saturating_sub(self.reader.stream_position()?);
        if size > remaining {
            return Err(Error::ChunkPastEnd);
        }
        let mut buf = vec![0u8; size as usize];
        self.reader.read_exact(&mut buf)?;
        Ok(buf)
    }
}

fn f32_at(b: &[u8], offset: usize) -> f32 {
    b.f32_at(offset).unwrap_or(0.0)
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn chunk(out: &mut Vec<u8>, magic: [u8; 4], payload: &[u8]) {
        out.extend_from_slice(&magic);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
    }

    fn map_with_tiles(tiles: &[(usize, usize)], trailing: bool) -> Vec<u8> {
        let mut out = Vec::new();
        chunk(&mut out, *b"REVM", &18u32.to_le_bytes());
        chunk(&mut out, *b"DHPM", &[0u8; 32]);
        let mut main = vec![0u8; MAIN_BYTES];
        for &(x, y) in tiles {
            main[(y * TILES_PER_MAP + x) * MAIN_ENTRY] = 0x1;
        }
        chunk(&mut out, *b"NIAM", &main);
        if trailing {
            chunk(&mut out, *b"FOOB", &[0xAB; 16]);
        }
        out
    }

    fn terrainless_map(flagged: bool, with_modf: bool) -> Vec<u8> {
        let mut out = Vec::new();
        chunk(&mut out, *b"REVM", &18u32.to_le_bytes());
        let mut mphd = [0u8; 32];
        mphd[0] = u8::from(flagged);
        chunk(&mut out, *b"DHPM", &mphd);
        chunk(&mut out, *b"NIAM", &vec![0u8; MAIN_BYTES]);
        chunk(&mut out, *b"OMWM", b"world\\wmo\\dungeon\\test\\t.wmo\0");
        if with_modf {
            let mut e = [0u8; 64];
            e[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
            for (i, v) in [11.0f32, 22.0, 33.0, 1.0, 180.0, 2.0].iter().enumerate() {
                e[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
            }
            e[56..58].copy_from_slice(&7u16.to_le_bytes());
            e[58..60].copy_from_slice(&3u16.to_le_bytes());
            e[60..62].copy_from_slice(&5u16.to_le_bytes());
            chunk(&mut out, *b"FDOM", &e);
        }
        out
    }

    fn parse(bytes: Vec<u8>) -> Result<WdtFile, Error> {
        WdtReader::new(Cursor::new(bytes)).read()
    }

    #[test]
    fn reads_the_global_wmo_of_a_terrainless_map() {
        let wdt = parse(terrainless_map(true, true)).expect("parses");
        let g = wdt.global_wmo().expect("a global WMO");
        assert_eq!(g.model, "world\\wmo\\dungeon\\test\\t.wmo");
        assert_eq!(g.position, [11.0, 22.0, 33.0]);
        assert_eq!(g.rotation, [1.0, 180.0, 2.0]);
        assert_eq!((g.doodad_set, g.name_set), (3, 5));
        assert!(!wdt.get_tile(32, 32).expect("in range").has_adt);
    }

    #[test]
    fn a_global_wmo_needs_the_flag_the_path_and_the_placement() {
        let terrain = parse(map_with_tiles(&[(30, 30)], false)).expect("parses");
        assert!(terrain.global_wmo().is_none());
        let unplaced = parse(terrainless_map(true, false)).expect("parses");
        assert!(unplaced.global_wmo().is_none());
        let unflagged = parse(terrainless_map(false, true)).expect("parses");
        assert!(unflagged.global_wmo().is_none());
    }

    #[test]
    fn reads_the_tile_table() {
        let wdt = parse(map_with_tiles(&[(3, 5), (63, 0), (0, 63)], false)).expect("parses");
        for (x, y) in [(3, 5), (63, 0), (0, 63)] {
            assert!(wdt.get_tile(x, y).expect("in range").has_adt, "({x},{y})");
        }
        let empty = wdt.get_tile(10, 10).expect("in range");
        assert!(!empty.has_adt);
        assert_eq!((empty.x, empty.y), (10, 10));
    }

    #[test]
    fn skips_unknown_chunks_before_and_after_main() {
        let wdt = parse(map_with_tiles(&[(1, 1)], true)).expect("parses");
        assert!(wdt.get_tile(1, 1).expect("in range").has_adt);
    }

    #[test]
    fn get_tile_off_the_grid_is_none() {
        let wdt = parse(map_with_tiles(&[], false)).expect("parses");
        assert!(wdt.get_tile(64, 0).is_none());
        assert!(wdt.get_tile(0, 64).is_none());
        assert!(wdt.get_tile(usize::MAX, usize::MAX).is_none());
    }

    #[test]
    fn a_file_without_main_is_an_error() {
        let mut only_mver = Vec::new();
        chunk(&mut only_mver, *b"REVM", &18u32.to_le_bytes());
        assert!(matches!(parse(only_mver), Err(Error::MissingMain)));
        assert!(matches!(parse(Vec::new()), Err(Error::MissingMain)));
    }

    #[test]
    fn a_partial_chunk_header_is_an_error() {
        assert!(matches!(
            parse(b"NIA".to_vec()),
            Err(Error::TruncatedHeader)
        ));
    }

    #[test]
    fn a_chunk_past_the_end_is_refused_before_allocating() {
        for magic in [b"DHPM", b"OMWM", b"FDOM"] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(magic);
            bytes.extend_from_slice(&u32::MAX.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 64]);
            assert!(matches!(parse(bytes), Err(Error::ChunkPastEnd)));
        }
    }
}
