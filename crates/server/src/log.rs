use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use protocol::ClientMessage;

use crate::world::{Input, Spawn, Stamped};

const MAGIC: &[u8; 12] = b"cairn-inputs";
const VERSION: u16 = 0;
const JOIN: u8 = 1;
const CLAIM: u8 = 2;
const LEAVE: u8 = 3;

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub tick_ms: u16,
    pub check: bool,
    pub spawns: Vec<Spawn>,
}

pub struct LogWriter {
    out: BufWriter<File>,
    buf: Vec<u8>,
}

impl LogWriter {
    pub fn create(path: &Path, header: &Header) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(MAGIC)?;
        out.write_all(&VERSION.to_le_bytes())?;
        out.write_all(&header.tick_ms.to_le_bytes())?;
        out.write_all(&[u8::from(header.check)])?;
        out.write_all(&(header.spawns.len() as u32).to_le_bytes())?;
        for s in &header.spawns {
            for v in [s.pos[0], s.pos[1], s.pos[2], s.facing] {
                out.write_all(&v.to_le_bytes())?;
            }
        }
        Ok(Self {
            out,
            buf: Vec::new(),
        })
    }

    pub fn tick(&mut self, tick: u32, applied: &[Stamped], hash_after: u64) -> io::Result<()> {
        self.buf.clear();
        self.buf.extend_from_slice(&tick.to_le_bytes());
        self.buf
            .extend_from_slice(&(applied.len() as u32).to_le_bytes());
        for s in applied {
            for v in [s.conn, s.nth, s.received_ms] {
                self.buf.extend_from_slice(&v.to_le_bytes());
            }
            match &s.input {
                Input::Join(h) => {
                    self.buf.push(JOIN);
                    ClientMessage::Hello(h.clone()).write(&mut self.buf);
                }
                Input::Claim(c) => {
                    self.buf.push(CLAIM);
                    ClientMessage::Claim(*c).write(&mut self.buf);
                }
                Input::Leave => self.buf.push(LEAVE),
            }
        }
        self.buf.extend_from_slice(&hash_after.to_le_bytes());
        self.out.write_all(&self.buf)
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.out.flush()
    }
}

pub struct LogReader {
    input: BufReader<File>,
    pub header: Header,
}

pub struct LoggedTick {
    pub tick: u32,
    pub inputs: Vec<Stamped>,
    pub hash: u64,
}

fn bad(what: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.to_string())
}

impl LogReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut input = BufReader::new(File::open(path)?);
        let mut magic = [0; 12];
        input.read_exact(&mut magic)?;
        if &magic != MAGIC || read_u16(&mut input)? != VERSION {
            return Err(bad("not an input log this server writes"));
        }
        let tick_ms = read_u16(&mut input)?;
        let check = read_u8(&mut input)? != 0;
        let n = read_u32(&mut input)?;
        let mut spawns = Vec::new();
        for _ in 0..n {
            let [x, y, z, facing]: [io::Result<f32>; 4] =
                std::array::from_fn(|_| read_f32(&mut input));
            let [x, y, z, facing] = [x?, y?, z?, facing?];
            spawns.push(Spawn {
                pos: [x, y, z],
                facing,
            });
        }
        Ok(Self {
            input,
            header: Header {
                tick_ms,
                check,
                spawns,
            },
        })
    }

    pub fn next_tick(&mut self) -> io::Result<Option<LoggedTick>> {
        let tick = match read_u32(&mut self.input) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        };
        let n = read_u32(&mut self.input)?;
        let mut inputs = Vec::with_capacity(n.min(1 << 20) as usize);
        for _ in 0..n {
            let (conn, nth, received_ms) = (
                read_u32(&mut self.input)?,
                read_u32(&mut self.input)?,
                read_u32(&mut self.input)?,
            );
            let input = match read_u8(&mut self.input)? {
                LEAVE => Input::Leave,
                JOIN | CLAIM => match ClientMessage::read(&self.frame()?) {
                    Ok(ClientMessage::Hello(h)) => Input::Join(h),
                    Ok(ClientMessage::Claim(c)) => Input::Claim(c),
                    Ok(ClientMessage::Seen(_)) | Err(_) => {
                        return Err(bad("a logged message is not a hello or a claim"));
                    }
                },
                _ => return Err(bad("an unknown input tag")),
            };
            inputs.push(Stamped {
                conn,
                nth,
                received_ms,
                input,
            });
        }
        let mut hash = [0; 8];
        self.input.read_exact(&mut hash)?;
        Ok(Some(LoggedTick {
            tick,
            inputs,
            hash: u64::from_le_bytes(hash),
        }))
    }

    fn frame(&mut self) -> io::Result<Vec<u8>> {
        let len = read_u32(&mut self.input)? as usize;
        if len > protocol::MAX_FRAME {
            return Err(bad("a logged frame is too long"));
        }
        let mut frame = vec![0; len];
        self.input.read_exact(&mut frame)?;
        Ok(frame)
    }
}

fn read_u8(r: &mut impl Read) -> io::Result<u8> {
    let mut b = [0; 1];
    r.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16(r: &mut impl Read) -> io::Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_f32(r: &mut impl Read) -> io::Result<f32> {
    read_u32(r).map(f32::from_bits)
}
