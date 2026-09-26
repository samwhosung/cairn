use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::crypto::{HashType, encrypt, hash_string};

const HASH_SLOTS: usize = 8;
const SECTOR_SHIFT: u16 = 0;
pub(crate) const SECTOR_SIZE: usize = 512 << SECTOR_SHIFT;

pub(crate) struct Entry<'a> {
    pub(crate) name: &'a str,
    pub(crate) flags: u32,
    pub(crate) data: &'a [u8],
    pub(crate) unpacked_size: u32,
}

impl<'a> Entry<'a> {
    pub(crate) fn new(name: &'a str, flags: u32, data: &'a [u8]) -> Self {
        Self {
            name,
            flags,
            data,
            unpacked_size: data.len() as u32,
        }
    }
}

pub(crate) fn header(hash_pos: u32, block_pos: u32, hash_len: u32, block_len: u32) -> Vec<u8> {
    let mut header = vec![0u8; 32];
    header[..4].copy_from_slice(b"MPQ\x1a");
    header[14..16].copy_from_slice(&SECTOR_SHIFT.to_le_bytes());
    for (at, value) in [
        (16, hash_pos),
        (20, block_pos),
        (24, hash_len),
        (28, block_len),
    ] {
        header[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    header
}

pub(crate) fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let hash_pos = 32;
    let block_pos = hash_pos + HASH_SLOTS * 16;
    let mut data_pos = block_pos + entries.len() * 16;
    let mut hash = vec![u32::MAX; HASH_SLOTS * 4];
    let mut block = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let mut slot = hash_string(entry.name, HashType::TableOffset) as usize % HASH_SLOTS;
        while hash[slot * 4 + 3] != u32::MAX {
            slot = (slot + 1) % HASH_SLOTS;
        }
        hash[slot * 4..slot * 4 + 4].copy_from_slice(&[
            hash_string(entry.name, HashType::NameA),
            hash_string(entry.name, HashType::NameB),
            0,
            index as u32,
        ]);
        block.extend([
            data_pos as u32,
            entry.data.len() as u32,
            entry.unpacked_size,
            entry.flags,
        ]);
        data_pos += entry.data.len();
    }
    encrypt(&mut hash, hash_string("(hash table)", HashType::FileKey));
    encrypt(&mut block, hash_string("(block table)", HashType::FileKey));
    let mut bytes = header(
        hash_pos as u32,
        block_pos as u32,
        HASH_SLOTS as u32,
        entries.len() as u32,
    );
    for word in hash.into_iter().chain(block) {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for entry in entries {
        bytes.extend_from_slice(entry.data);
    }
    bytes
}

pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mpq-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create a temp dir");
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create a temp file's directory");
        }
        std::fs::write(&path, bytes).expect("write a temp file");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
