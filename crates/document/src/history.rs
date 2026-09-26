//! `history.bin`: for each command of the journal, its footprint's image from before it and a
//! digest of its image after; for each undo, the images it took back, which a redo puts back.
//! A record belongs to a journal line and is written just before it, so a record whose line
//! never made it into the journal is dropped when the zone opens.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::choice::hash_bytes;
use crate::image::{Image, Reader};

pub const FILE: &str = "history.bin";
const HEADER: usize = 1 + 4 + 4 + 8;
const FORWARD: u8 = 1;
const TAKEN_BACK: u8 = 2;

pub struct CommandRecord {
    pub before: Image,
    pub after_digest: u64,
}

#[derive(Clone, Copy)]
struct Stored {
    kind: u8,
    offset: u64,
    len: usize,
}

pub struct History {
    path: PathBuf,
    file: File,
    by_line: BTreeMap<usize, Stored>,
}

impl History {
    pub fn create(dir: &Path) -> Result<History, String> {
        let path = dir.join(FILE);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(History {
            path,
            file,
            by_line: BTreeMap::new(),
        })
    }

    /// Open `history.bin`, keeping the records of the journal's first `lines` lines.
    pub fn open(dir: &Path, lines: usize) -> Result<History, String> {
        let path = dir.join(FILE);
        let io = |e: std::io::Error| format!("{}: {e}", path.display());
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .map_err(io)?;
        let size = file.metadata().map_err(io)?.len();
        let mut by_line = BTreeMap::new();
        let mut at = 0u64;
        let mut header = [0u8; HEADER];
        while at + HEADER as u64 <= size {
            file.seek(SeekFrom::Start(at)).map_err(io)?;
            file.read_exact(&mut header).map_err(io)?;
            let mut r = Reader { b: &header, at: 0 };
            let (kind, line, len) = (r.u8()?, r.u32()? as usize, r.u32()? as usize);
            let end = at + (HEADER + len) as u64;
            if line > lines || end > size || !matches!(kind, FORWARD | TAKEN_BACK) {
                break;
            }
            by_line.insert(
                line,
                Stored {
                    kind,
                    offset: at,
                    len,
                },
            );
            at = end;
        }
        if at < size {
            file.set_len(at).map_err(io)?;
        }
        file.seek(SeekFrom::End(0)).map_err(io)?;
        Ok(History {
            path,
            file,
            by_line,
        })
    }

    fn write(&mut self, kind: u8, line: usize, body: &[u8]) -> Result<(), String> {
        let at = self
            .file
            .seek(SeekFrom::End(0))
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        let mut b = Vec::with_capacity(HEADER + body.len());
        b.push(kind);
        b.extend_from_slice(&(line as u32).to_le_bytes());
        b.extend_from_slice(&(body.len() as u32).to_le_bytes());
        b.extend_from_slice(&hash_bytes(body).to_le_bytes());
        b.extend_from_slice(body);
        self.file
            .write_all(&b)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        self.by_line.insert(
            line,
            Stored {
                kind,
                offset: at,
                len: body.len(),
            },
        );
        Ok(())
    }

    fn body(&mut self, line: usize, want: u8) -> Result<Vec<u8>, String> {
        let Stored { kind, offset, len } = *self
            .by_line
            .get(&line)
            .ok_or_else(|| format!("{}: no record of journal line {line}", self.path.display()))?;
        if kind != want {
            return Err(format!(
                "{}: line {line}'s record is of another kind",
                self.path.display()
            ));
        }
        let io = |e: std::io::Error| format!("{}: {e}", self.path.display());
        let mut b = vec![0u8; HEADER + len];
        self.file.seek(SeekFrom::Start(offset)).map_err(io)?;
        self.file.read_exact(&mut b).map_err(io)?;
        self.file.seek(SeekFrom::End(0)).map_err(io)?;
        let sum = u64::from_le_bytes(b[9..17].try_into().unwrap_or_default());
        let body = b.split_off(HEADER);
        if hash_bytes(&body) != sum {
            return Err(format!(
                "{}: the record of journal line {line} is damaged",
                self.path.display()
            ));
        }
        Ok(body)
    }

    pub fn write_forward(&mut self, line: usize, f: &CommandRecord) -> Result<(), String> {
        let mut body = f.after_digest.to_le_bytes().to_vec();
        f.before.encode(&mut body);
        self.write(FORWARD, line, &body)
    }

    pub fn forward(&mut self, line: usize) -> Result<CommandRecord, String> {
        let body = self.body(line, FORWARD)?;
        let mut r = Reader { b: &body, at: 0 };
        let after_digest = r.u64()?;
        Ok(CommandRecord {
            before: Image::decode(&body[8..])?,
            after_digest,
        })
    }

    pub fn write_taken_back(&mut self, line: usize, images: &[Image]) -> Result<(), String> {
        let mut body = (images.len() as u32).to_le_bytes().to_vec();
        for i in images {
            let mut b = Vec::new();
            i.encode(&mut b);
            body.extend_from_slice(&(b.len() as u32).to_le_bytes());
            body.extend_from_slice(&b);
        }
        self.write(TAKEN_BACK, line, &body)
    }

    pub fn taken_back(&mut self, line: usize) -> Result<Vec<Image>, String> {
        let body = self.body(line, TAKEN_BACK)?;
        let mut r = Reader { b: &body, at: 0 };
        let mut out = Vec::new();
        for _ in 0..r.count()? {
            let n = r.count()?;
            out.push(Image::decode(r.bytes(n)?)?);
        }
        Ok(out)
    }
}
