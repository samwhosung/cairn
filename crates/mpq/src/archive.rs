use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;

use crate::crypto::{HashType, decrypt, hash_string};
use crate::error::Error;

const SIGNATURE: u32 = u32::from_le_bytes(*b"MPQ\x1a");
const USER_DATA_SIGNATURE: u32 = u32::from_le_bytes(*b"MPQ\x1b");
const HEADER_ALIGN: u64 = 512;
const HEADER_LEN: usize = 32;
/// `512 << 23` bytes is 4 GiB, more than any file's `u32` size.
const MAX_SECTOR_SHIFT: u16 = 23;

const FLAG_IMPLODE: u32 = 0x0000_0100;
const FLAG_COMPRESS: u32 = 0x0000_0200;
const FLAG_ENCRYPTED: u32 = 0x0001_0000;
const FLAG_SINGLE_UNIT: u32 = 0x0100_0000;
pub(crate) const FLAG_DELETE_MARKER: u32 = 0x0200_0000;
pub(crate) const FLAG_EXISTS: u32 = 0x8000_0000;

const EMPTY_SLOT: u32 = 0xFFFF_FFFF;
const DELETED_SLOT: u32 = 0xFFFF_FFFE;

const CODEC_ZLIB: u8 = 0x02;
const CODEC_IMPLODE: u8 = 0x08;

struct HashEntry {
    name_a: u32,
    name_b: u32,
    block_index: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct BlockEntry {
    pos: u32,
    pub(crate) size: u32,
    flags: u32,
}

impl BlockEntry {
    pub(crate) fn is_delete_marker(self) -> bool {
        self.flags & FLAG_DELETE_MARKER != 0
    }
}

/// An open MPQ archive, its tables read once. Names ignore ASCII case, and `/` equals `\`.
pub struct Archive {
    path: PathBuf,
    header_pos: u64,
    sector_size: usize,
    hash_table: Vec<HashEntry>,
    block_table: Vec<BlockEntry>,
}

impl Archive {
    /// Opens the archive at `path` and reads its hash and block tables.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_len = file.metadata()?.len();
        let header_pos = find_header(&mut file, file_len)?;
        // Later format versions only extend this header; 1.12.1 needs nothing past it.
        let mut header = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(header_pos))?;
        file.read_exact(&mut header)?;
        let word = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        if word(0) != SIGNATURE {
            return Err(Error::NotMpq);
        }
        let sector_shift = u16::from_le_bytes([header[14], header[15]]);
        let hash_pos = header_pos + u64::from(word(16));
        let hash_table = read_table(&mut file, file_len, hash_pos, word(24), "(hash table)")?
            .into_iter()
            .map(|[name_a, name_b, _locale, block_index]| HashEntry {
                name_a,
                name_b,
                block_index,
            })
            .collect();
        let block_pos = header_pos + u64::from(word(20));
        let block_table = read_table(&mut file, file_len, block_pos, word(28), "(block table)")?
            .into_iter()
            .map(|[pos, _packed_size, size, flags]| BlockEntry { pos, size, flags })
            .collect();
        Ok(Self {
            path,
            header_pos,
            sector_size: sector_size(sector_shift)?,
            hash_table,
            block_table,
        })
    }

    /// Whether the archive has an entry for `name`, a delete marker included.
    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The decompressed size of `name`, if the archive has an entry for it.
    pub fn file_size(&self, name: &str) -> Option<u32> {
        self.find(name).map(|entry| entry.size)
    }

    /// Whether the entry for `name` is a delete marker, which hides the path in every
    /// lower-priority archive and has no data of its own.
    pub fn is_delete_marker(&self, name: &str) -> bool {
        self.find(name).is_some_and(BlockEntry::is_delete_marker)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads and decompresses `name`. Each read opens its own file handle, so reads through a
    /// shared reference can run in parallel.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, Error> {
        let entry = self
            .find(name)
            .filter(|entry| entry.flags & FLAG_EXISTS != 0 && !entry.is_delete_marker())
            .ok_or_else(|| Error::NotFound(name.to_owned()))?;
        if entry.flags & FLAG_ENCRYPTED != 0 {
            return Err(Error::Unsupported(format!("encrypted file {name}")));
        }
        if entry.flags & FLAG_SINGLE_UNIT != 0 {
            return Err(Error::Unsupported(format!("single-unit file {name}")));
        }
        let mut file = File::open(&self.path)?;
        let pos = self.header_pos + u64::from(entry.pos);
        let avail = bytes_after(file.metadata()?.len(), pos);
        if entry.flags & (FLAG_COMPRESS | FLAG_IMPLODE) != 0 {
            return self.read_sectors(&mut file, name, entry, pos, avail);
        }
        let size = entry.size as usize;
        if size > avail {
            return Err(Error::Corrupt(format!(
                "{name}: stored size ({size}) larger than the archive"
            )));
        }
        read_at(&mut file, pos, size)
    }

    pub(crate) fn find(&self, name: &str) -> Option<BlockEntry> {
        if self.hash_table.is_empty() {
            return None;
        }
        let home = hash_string(name, HashType::TableOffset) as usize & (self.hash_table.len() - 1);
        let name_a = hash_string(name, HashType::NameA);
        let name_b = hash_string(name, HashType::NameB);
        let (wrapped, from_home) = self.hash_table.split_at(home);
        from_home
            .iter()
            .chain(wrapped)
            .take_while(|slot| slot.block_index != EMPTY_SLOT)
            .find(|slot| {
                slot.block_index != DELETED_SLOT && slot.name_a == name_a && slot.name_b == name_b
            })
            .and_then(|slot| self.block_table.get(slot.block_index as usize).copied())
    }

    fn read_sectors(
        &self,
        file: &mut File,
        name: &str,
        entry: BlockEntry,
        pos: u64,
        avail: usize,
    ) -> Result<Vec<u8>, Error> {
        let size = entry.size as usize;
        let imploded = entry.flags & FLAG_COMPRESS == 0;
        // Offsets count from the file's start. Files with sector checksums add one more offset
        // and a checksum table after the last sector; neither is read.
        let count = size.div_ceil(self.sector_size) + 1;
        if count > avail / 4 {
            return Err(Error::Corrupt(format!(
                "{name}: sector offset table ({count} entries) larger than the archive"
            )));
        }
        let offsets: Vec<u64> = read_at(file, pos, count * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|offset| u64::from(u32::from_le_bytes(*offset)))
            .collect();
        let mut out = Vec::with_capacity(size.min(avail));
        for (i, bounds) in offsets.windows(2).enumerate() {
            let len = bounds[1]
                .checked_sub(bounds[0])
                .ok_or_else(|| Error::Corrupt(format!("{name}: sector {i} offsets reversed")))?
                as usize;
            if len > avail {
                return Err(Error::Corrupt(format!(
                    "{name}: sector {i} length ({len}) larger than the archive"
                )));
            }
            if len == 0 {
                return Err(Error::Corrupt(format!("{name}: sector {i} is empty")));
            }
            let raw = read_at(file, pos + bounds[0], len)?;
            let want = (size - out.len()).min(self.sector_size);
            let before = out.len();
            if len >= want {
                // A sector that would not shrink is stored as is.
                out.extend_from_slice(&raw[..want]);
            } else if imploded {
                // Imploded files have no codec byte.
                decompress(CODEC_IMPLODE, &raw, want, name, &mut out)?;
            } else {
                decompress(raw[0], &raw[1..], want, name, &mut out)?;
            }
            if out.len() - before != want {
                return Err(Error::Decompress(format!(
                    "{name}: sector {i} inflated to {} of {want} bytes",
                    out.len() - before
                )));
            }
        }
        Ok(out)
    }
}

fn sector_size(shift: u16) -> Result<usize, Error> {
    if shift > MAX_SECTOR_SHIFT {
        return Err(Error::Corrupt(format!(
            "sector size shift {shift} is too large"
        )));
    }
    usize::try_from(512u64 << shift).map_err(|_| {
        Error::Corrupt(format!(
            "sector size shift {shift} is too large for this platform"
        ))
    })
}

fn find_header(file: &mut File, file_len: u64) -> Result<u64, Error> {
    let mut pos = 0;
    while pos + 4 <= file_len {
        let mut signature = [0u8; 4];
        file.seek(SeekFrom::Start(pos))?;
        if file.read_exact(&mut signature).is_err() {
            break;
        }
        match u32::from_le_bytes(signature) {
            SIGNATURE => return Ok(pos),
            USER_DATA_SIGNATURE => {
                let mut fields = [[0u8; 4]; 2];
                file.read_exact(fields.as_flattened_mut())?;
                let [_user_data_size, header_offset] = fields;
                return Ok(pos + u64::from(u32::from_le_bytes(header_offset)));
            }
            _ => pos += HEADER_ALIGN,
        }
    }
    Err(Error::NotMpq)
}

fn read_table(
    file: &mut File,
    file_len: u64,
    pos: u64,
    count: u32,
    key_name: &str,
) -> Result<Vec<[u32; 4]>, Error> {
    let count = count as usize;
    let room = bytes_after(file_len, pos) / 16;
    if count > room {
        return Err(Error::Corrupt(format!(
            "{key_name}: header claims {count} entries, only room for {room} in the archive"
        )));
    }
    let mut words: Vec<u32> = read_at(file, pos, count * 16)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes(*word))
        .collect();
    decrypt(&mut words, hash_string(key_name, HashType::FileKey));
    Ok(words.as_chunks::<4>().0.to_vec())
}

fn read_at(file: &mut File, pos: u64, len: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![0u8; len];
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn bytes_after(file_len: u64, pos: u64) -> usize {
    usize::try_from(file_len.saturating_sub(pos)).unwrap_or(usize::MAX)
}

fn decompress(
    codec: u8,
    data: &[u8],
    want: usize,
    name: &str,
    out: &mut Vec<u8>,
) -> Result<(), Error> {
    if codec != CODEC_ZLIB {
        return Err(Error::Unsupported(format!(
            "{name}: sector codec 0x{codec:02X} is not supported"
        )));
    }
    ZlibDecoder::new(data)
        .take(want as u64)
        .read_to_end(out)
        .map_err(|e| Error::Decompress(format!("{name}: zlib: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::ZlibEncoder;

    use super::*;
    use crate::fixture::{Entry, SECTOR_SIZE, TempDir, archive, header};

    fn open(bytes: &[u8]) -> (TempDir, Result<Archive, Error>) {
        let dir = TempDir::new();
        let archive = Archive::open(dir.write("test.MPQ", bytes));
        (dir, archive)
    }

    fn one_file(name: &str, flags: u32, data: &[u8]) -> Vec<u8> {
        archive(&[Entry::new(name, flags, data)])
    }

    #[test]
    fn open_rejects_a_hash_table_larger_than_the_file() {
        let mut bytes = header(32, 32, u32::MAX, 0);
        bytes.extend_from_slice(&[0; 8]);
        let (_dir, result) = open(&bytes);
        assert!(matches!(result, Err(Error::Corrupt(_))));
    }

    #[test]
    fn open_rejects_a_block_table_larger_than_the_file() {
        let mut bytes = header(32, 32, 0, u32::MAX);
        bytes.extend_from_slice(&[0; 8]);
        let (_dir, result) = open(&bytes);
        assert!(matches!(result, Err(Error::Corrupt(_))));
    }

    #[test]
    fn open_accepts_empty_tables_at_the_end_of_the_file() {
        let (_dir, result) = open(&header(32, 32, 0, 0));
        let archive = result.expect("an archive with empty tables opens");
        assert!(!archive.contains("anything"));
        assert_eq!(archive.file_size("anything"), None);
    }

    #[test]
    fn a_stored_file_reads_its_bytes() {
        let name = "Interface\\a.txt";
        let (_dir, result) = open(&one_file(name, FLAG_EXISTS, b"hello"));
        let archive = result.expect("open");
        assert!(archive.contains(name));
        assert!(!archive.is_delete_marker(name));
        assert_eq!(archive.read(name).expect("read"), b"hello");
    }

    #[test]
    fn a_delete_marker_is_not_a_readable_empty_file() {
        let name = "Interface\\FrameXML\\ClassTrainerFrame.xml";
        let (_dir, result) = open(&one_file(name, FLAG_EXISTS | FLAG_DELETE_MARKER, &[]));
        let archive = result.expect("open");
        assert!(archive.contains(name));
        assert!(archive.is_delete_marker(name));
        assert!(matches!(archive.read(name), Err(Error::NotFound(_))));
    }

    #[test]
    fn bytes_after_saturates_past_the_end() {
        assert_eq!(bytes_after(10, 100), 0);
        assert_eq!(bytes_after(100, 10), 90);
        assert_eq!(bytes_after(0, 0), 0);
    }

    #[test]
    fn reversed_sector_offsets_are_corrupt() {
        let offsets: Vec<u8> = [12u32, 8].iter().flat_map(|o| o.to_le_bytes()).collect();
        let (_dir, result) = open(&one_file("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &offsets));
        assert!(matches!(
            result.expect("open").read("a.bin"),
            Err(Error::Corrupt(_))
        ));
    }

    #[test]
    fn an_empty_sector_is_refused() {
        let offsets: Vec<u8> = [8u32, 8].iter().flat_map(|o| o.to_le_bytes()).collect();
        let (_dir, result) = open(&one_file("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &offsets));
        let archive = result.expect("open");
        assert!(matches!(archive.read("a.bin"), Err(Error::Corrupt(_))));
    }

    #[test]
    fn an_over_inflating_sector_stops_at_the_file_size() {
        let zlib = |raw: &[u8]| {
            let mut encoder = ZlibEncoder::new(vec![CODEC_ZLIB], Compression::default());
            encoder.write_all(raw).expect("compress");
            encoder.finish().expect("compress")
        };
        let twice_its_sector = zlib(&[0; 2 * SECTOR_SIZE]);
        let tail = zlib(&[7; 100]);
        let table_len = 12;
        let mut data = Vec::new();
        for offset in [
            table_len,
            table_len + twice_its_sector.len(),
            table_len + twice_its_sector.len() + tail.len(),
        ] {
            data.extend_from_slice(&(offset as u32).to_le_bytes());
        }
        data.extend_from_slice(&twice_its_sector);
        data.extend_from_slice(&tail);
        let file = Entry {
            unpacked_size: (SECTOR_SIZE + 100) as u32,
            ..Entry::new("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &data)
        };
        let (_dir, result) = open(&archive(&[file]));
        let out = result.expect("open").read("a.bin").expect("read");
        assert_eq!(out.len(), SECTOR_SIZE + 100);
        assert!(out[..SECTOR_SIZE].iter().all(|&b| b == 0));
        assert!(out[SECTOR_SIZE..].iter().all(|&b| b == 7));
    }

    #[test]
    fn sector_sizes_never_wrap() {
        assert_eq!(sector_size(3).ok(), Some(4096));
        assert!(matches!(
            sector_size(MAX_SECTOR_SHIFT + 1),
            Err(Error::Corrupt(_))
        ));
        let largest = sector_size(MAX_SECTOR_SHIFT);
        if usize::BITS > 32 {
            assert_eq!(largest.ok(), Some(512 << MAX_SECTOR_SHIFT));
        } else {
            assert!(matches!(largest, Err(Error::Corrupt(_))));
        }
    }

    #[test]
    fn a_sector_that_inflates_short_is_refused() {
        let zlib = |raw: &[u8]| {
            let mut encoder = ZlibEncoder::new(vec![CODEC_ZLIB], Compression::default());
            encoder.write_all(raw).expect("compress");
            encoder.finish().expect("compress")
        };
        let short = zlib(&[0; 10]);
        let tail = zlib(&[7; 100]);
        let table_len = 12;
        let mut data = Vec::new();
        for offset in [
            table_len,
            table_len + short.len(),
            table_len + short.len() + tail.len(),
        ] {
            data.extend_from_slice(&(offset as u32).to_le_bytes());
        }
        data.extend_from_slice(&short);
        data.extend_from_slice(&tail);
        let file = Entry {
            unpacked_size: (SECTOR_SIZE + 100) as u32,
            ..Entry::new("a.bin", FLAG_EXISTS | FLAG_COMPRESS, &data)
        };
        let (_dir, result) = open(&archive(&[file]));
        assert!(matches!(
            result.expect("open").read("a.bin"),
            Err(Error::Decompress(_))
        ));
    }

    #[test]
    fn a_stored_size_larger_than_the_archive_is_refused_before_allocating() {
        let file = Entry {
            unpacked_size: u32::MAX,
            ..Entry::new("a.txt", FLAG_EXISTS, b"hello")
        };
        let (_dir, result) = open(&archive(&[file]));
        let archive = result.expect("open");
        assert!(matches!(archive.read("a.txt"), Err(Error::Corrupt(_))));
    }
}
