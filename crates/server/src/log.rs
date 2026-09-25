use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use game::{KnobsFile, Line};
use protocol::ClientMessage;

use crate::save::{Place, Player};
use crate::world::{Input, Spawn, Stamped};

const MAGIC: &[u8; 12] = b"cairn-inputs";
const VERSION: u16 = 1;
const JOIN: u8 = 1;
const CLAIM: u8 = 2;
const LEAVE: u8 = 3;
const HOST_JOIN: u8 = 4;
const TELEPORT: u8 = 5;
const ACTION: u8 = 6;

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub tick_ms: u16,
    pub check: bool,
    pub map: u32,
    pub spawns: Vec<Spawn>,
    pub game: Option<Logged>,
    /// The players the world knew as the run began.
    pub players: Vec<Player>,
}

/// The game a run played, as it was loaded.
#[derive(Clone, Debug, PartialEq)]
pub struct Logged {
    pub name: String,
    pub seed: u64,
    pub knobs: KnobsFile,
    pub overlay: Vec<Line>,
}

pub struct LogWriter {
    out: BufWriter<File>,
    buf: Vec<u8>,
}

impl LogWriter {
    pub fn create(path: &Path, header: &Header) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&VERSION.to_le_bytes());
        b.extend_from_slice(&header.tick_ms.to_le_bytes());
        b.push(u8::from(header.check));
        b.extend_from_slice(&header.map.to_le_bytes());
        b.extend_from_slice(&(header.spawns.len() as u32).to_le_bytes());
        for s in &header.spawns {
            for v in [s.pos[0], s.pos[1], s.pos[2], s.facing] {
                b.extend_from_slice(&v.to_le_bytes());
            }
        }
        b.push(u8::from(header.game.is_some()));
        if let Some(g) = &header.game {
            put_text(&mut b, &g.name);
            b.extend_from_slice(&g.seed.to_le_bytes());
            put_text(&mut b, &g.knobs.name);
            put_lines(&mut b, &g.knobs.lines);
            put_lines(&mut b, &g.overlay);
        }
        b.extend_from_slice(&(header.players.len() as u32).to_le_bytes());
        for p in &header.players {
            b.extend_from_slice(&p.id.to_le_bytes());
            put_text(&mut b, &p.name);
            b.push(u8::from(p.place.is_some()));
            if let Some(at) = p.place {
                b.extend_from_slice(&at.map.to_le_bytes());
                for v in [at.pos[0], at.pos[1], at.pos[2], at.facing] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
            }
            b.push(u8::from(p.saved.is_some()));
            if let Some(saved) = &p.saved {
                put_bytes(&mut b, saved);
            }
        }
        out.write_all(&b)?;
        out.flush()?;
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
            let (tag, message) = match &s.input {
                Input::Join(h) => (JOIN, ClientMessage::Hello(h.clone())),
                Input::HostJoin(h) => (HOST_JOIN, ClientMessage::Hello(h.clone())),
                Input::Claim(c) => (CLAIM, ClientMessage::Claim(*c)),
                Input::Teleport(c) => (TELEPORT, ClientMessage::Teleport(*c)),
                Input::Action(number) => (ACTION, ClientMessage::Action(*number)),
                Input::Leave => {
                    self.buf.push(LEAVE);
                    continue;
                }
            };
            self.buf.push(tag);
            message.write(&mut self.buf);
        }
        self.buf.extend_from_slice(&hash_after.to_le_bytes());
        self.out.write_all(&self.buf)?;
        self.out.flush()
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
        let map = read_u32(&mut input)?;
        let n = read_u32(&mut input)?;
        let mut spawns = Vec::new();
        for _ in 0..n {
            let [x, y, z, facing] = read_spot(&mut input)?;
            spawns.push(Spawn {
                pos: [x, y, z],
                facing,
            });
        }
        let game = read_some(&mut input, |r| {
            Ok(Logged {
                name: read_text(r)?,
                seed: read_u64(r)?,
                knobs: KnobsFile {
                    name: read_text(r)?,
                    lines: read_lines(r)?,
                },
                overlay: read_lines(r)?,
            })
        })?;
        let n = read_u32(&mut input)?;
        let mut players = Vec::new();
        for _ in 0..n {
            let (id, name) = (read_u32(&mut input)?, read_text(&mut input)?);
            let place = read_some(&mut input, |r| {
                let map = read_u32(r)?;
                let [x, y, z, facing] = read_spot(r)?;
                Ok(Place {
                    map,
                    pos: [x, y, z],
                    facing,
                })
            })?;
            let saved = read_some(&mut input, read_bytes)?;
            players.push(Player {
                id,
                name,
                place,
                saved,
            });
        }
        Ok(Self {
            input,
            header: Header {
                tick_ms,
                check,
                map,
                spawns,
                game,
                players,
            },
        })
    }

    /// The next tick in the log; `None` at its end, and at a tick a crash cut off.
    pub fn next_tick(&mut self) -> io::Result<Option<LoggedTick>> {
        match self.whole_tick() {
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),
            tick => tick.map(Some),
        }
    }

    fn whole_tick(&mut self) -> io::Result<LoggedTick> {
        let tick = read_u32(&mut self.input)?;
        let n = read_u32(&mut self.input)?;
        let mut inputs = Vec::with_capacity(n.min(1 << 20) as usize);
        for _ in 0..n {
            let (conn, nth, received_ms) = (
                read_u32(&mut self.input)?,
                read_u32(&mut self.input)?,
                read_u32(&mut self.input)?,
            );
            let tag = read_u8(&mut self.input)?;
            let input = match tag {
                LEAVE => Input::Leave,
                JOIN | HOST_JOIN | CLAIM | TELEPORT | ACTION => {
                    match (tag, ClientMessage::read(&self.frame()?)) {
                        (JOIN, Ok(ClientMessage::Hello(h))) => Input::Join(h),
                        (HOST_JOIN, Ok(ClientMessage::Hello(h))) => Input::HostJoin(h),
                        (CLAIM, Ok(ClientMessage::Claim(c))) => Input::Claim(c),
                        (TELEPORT, Ok(ClientMessage::Teleport(c))) => Input::Teleport(c),
                        (ACTION, Ok(ClientMessage::Action(number))) => Input::Action(number),
                        _ => return Err(bad("a logged message is not what its tag says")),
                    }
                }
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
        Ok(LoggedTick {
            tick,
            inputs,
            hash: u64::from_le_bytes(hash),
        })
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

fn put_bytes(b: &mut Vec<u8>, bytes: &[u8]) {
    b.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    b.extend_from_slice(bytes);
}

fn put_text(b: &mut Vec<u8>, text: &str) {
    put_bytes(b, text.as_bytes());
}

fn put_lines(b: &mut Vec<u8>, lines: &[Line]) {
    b.extend_from_slice(&(lines.len() as u32).to_le_bytes());
    for l in lines {
        for text in [&l.key, &l.value, &l.at] {
            put_text(b, text);
        }
    }
}

fn read_bytes(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let len = read_u32(r)? as usize;
    if len > protocol::MAX_FRAME {
        return Err(bad("a logged value is too long"));
    }
    let mut bytes = vec![0; len];
    r.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_text(r: &mut impl Read) -> io::Result<String> {
    String::from_utf8(read_bytes(r)?).map_err(|_| bad("logged text is not UTF-8"))
}

fn read_lines(r: &mut impl Read) -> io::Result<Vec<Line>> {
    let n = read_u32(r)?;
    let mut lines = Vec::new();
    for _ in 0..n {
        let (key, value, at) = (read_text(r)?, read_text(r)?, read_text(r)?);
        lines.push(Line { key, value, at });
    }
    Ok(lines)
}

fn read_some<R: Read, T>(
    r: &mut R,
    read: impl FnOnce(&mut R) -> io::Result<T>,
) -> io::Result<Option<T>> {
    if read_u8(r)? == 0 {
        Ok(None)
    } else {
        read(r).map(Some)
    }
}

fn read_spot(r: &mut impl Read) -> io::Result<[f32; 4]> {
    Ok([read_f32(r)?, read_f32(r)?, read_f32(r)?, read_f32(r)?])
}

fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
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
