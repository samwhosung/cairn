use crate::batch::Batch;
use crate::frame::{Kind, begin_frame, finish_frame};
use crate::reader::Reader;
use crate::{Appearance, Error, Movement};

pub(crate) const MAX_NAME: usize = u8::MAX as usize;

/// What a client says first: which protocol it speaks, who it is and what it looks like.
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    pub version: u16,
    pub name: String,
    pub appearance: Appearance,
}

/// Where a client says its mover is now. `ack` is the sequence number of the last correction
/// the client has taken; the server ignores claims that do not carry its latest one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Claim {
    pub ack: u32,
    pub movement: Movement,
}

impl Claim {
    fn write(&self, out: &mut Vec<u8>, kind: Kind) {
        let start = begin_frame(out, kind);
        out.extend_from_slice(&self.ack.to_le_bytes());
        self.movement.write(out);
        finish_frame(out, start);
    }

    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self {
            ack: r.u32()?,
            movement: Movement::read(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClientMessage {
    Hello(Hello),
    Claim(Claim),
    /// The latest tick whose batch the client has taken in, so the server can tell how far
    /// behind it is wherever the bytes between them wait.
    Seen(u32),
    /// Where the client asks its mover to be put: somewhere its movement did not take it.
    Teleport(Claim),
}

impl ClientMessage {
    /// Appends this message as one frame.
    pub fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hello(h) => {
                let start = begin_frame(out, Kind::Hello);
                out.extend_from_slice(&h.version.to_le_bytes());
                write_name(out, &h.name);
                h.appearance.write(out);
                finish_frame(out, start);
            }
            Self::Claim(c) => c.write(out, Kind::Claim),
            Self::Teleport(c) => c.write(out, Kind::Teleport),
            Self::Seen(tick) => {
                let start = begin_frame(out, Kind::Seen);
                out.extend_from_slice(&tick.to_le_bytes());
                finish_frame(out, start);
            }
        }
    }

    /// Reads one frame's body, kind byte first, as [`crate::Frames`] hands it out.
    pub fn read(frame: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(frame);
        let msg = match Kind::of(r.u8()?)? {
            Kind::Hello => Self::Hello(Hello {
                version: r.u16()?,
                name: read_name(&mut r)?.to_owned(),
                appearance: Appearance::read(&mut r)?,
            }),
            Kind::Claim => Self::Claim(Claim::read(&mut r)?),
            Kind::Teleport => Self::Teleport(Claim::read(&mut r)?),
            Kind::Seen => Self::Seen(r.u32()?),
            other => return Err(Error::Unexpected(other as u8)),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// The server's answer to a hello: the client's entity id, the map, the tick clock, and where
/// its mover stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Welcome {
    pub version: u16,
    pub id: u32,
    /// A `Map.dbc` id.
    pub map: u32,
    /// The tick the first batch will carry.
    pub tick: u32,
    pub tick_ms: u16,
    pub spawn: Movement,
}

impl Welcome {
    /// Appends this message as one frame.
    pub fn write(&self, out: &mut Vec<u8>) {
        let start = begin_frame(out, Kind::Welcome);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.id.to_le_bytes());
        out.extend_from_slice(&self.map.to_le_bytes());
        out.extend_from_slice(&self.tick.to_le_bytes());
        out.extend_from_slice(&self.tick_ms.to_le_bytes());
        self.spawn.write(out);
        finish_frame(out, start);
    }
}

pub enum ServerMessage<'a> {
    Welcome(Welcome),
    Batch(Batch<'a>),
}

impl<'a> ServerMessage<'a> {
    /// Reads one frame's body, kind byte first. A batch's records are read as it is iterated.
    pub fn read(frame: &'a [u8]) -> Result<Self, Error> {
        let mut r = Reader::new(frame);
        match Kind::of(r.u8()?)? {
            Kind::Welcome => {
                let welcome = Welcome {
                    version: r.u16()?,
                    id: r.u32()?,
                    map: r.u32()?,
                    tick: r.u32()?,
                    tick_ms: r.u16()?,
                    spawn: Movement::read(&mut r)?,
                };
                r.finish()?;
                Ok(Self::Welcome(welcome))
            }
            Kind::Batch => Ok(Self::Batch(Batch::read(r)?)),
            other => Err(Error::Unexpected(other as u8)),
        }
    }
}

pub(crate) fn write_name(out: &mut Vec<u8>, name: &str) {
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    out.push(end as u8);
    out.extend_from_slice(&name.as_bytes()[..end]);
}

pub(crate) fn read_name<'a>(r: &mut Reader<'a>) -> Result<&'a str, Error> {
    let len = usize::from(r.u8()?);
    std::str::from_utf8(r.bytes(len)?).map_err(|_| Error::Name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frames, flags};

    fn one_frame(bytes: &[u8]) -> Vec<u8> {
        let mut frames = Frames::default();
        frames.extend(bytes);
        let frame = frames.next_frame().expect("valid").expect("whole").to_vec();
        assert_eq!(frames.pending(), 0);
        frame
    }

    #[test]
    fn client_messages_round_trip() {
        let hello = ClientMessage::Hello(Hello {
            version: crate::VERSION,
            name: "Brother Sammuel".into(),
            appearance: Appearance {
                race: 1,
                sex: 1,
                skin: 3,
                hair_style: 5,
                equipment: [0, 0, 0, 1234, 0, 0, 0, 0, 0, 99],
                ..Appearance::default()
            },
        });
        let claim = Claim {
            ack: 7,
            movement: Movement {
                time: 1000,
                flags: flags::FORWARD,
                pos: [1.0, 2.0, 3.0],
                facing: 0.5,
                ..Movement::default()
            },
        };
        for msg in [
            hello,
            ClientMessage::Claim(claim),
            ClientMessage::Seen(77),
            ClientMessage::Teleport(claim),
        ] {
            let mut out = Vec::new();
            msg.write(&mut out);
            assert_eq!(ClientMessage::read(&one_frame(&out)), Ok(msg));
        }
    }

    #[test]
    fn a_long_name_is_cut_at_a_character_boundary() {
        let name = "é".repeat(200);
        let mut out = Vec::new();
        write_name(&mut out, &name);
        let got = read_name(&mut Reader::new(&out)).expect("utf-8");
        assert_eq!(got.len(), 254);
        assert!(name.starts_with(got));
    }

    #[test]
    fn a_welcome_round_trips_and_a_batch_is_not_for_the_server() {
        let welcome = Welcome {
            version: crate::VERSION,
            id: 42,
            map: 0,
            tick: 9000,
            tick_ms: 50,
            spawn: Movement {
                pos: [-9439.1, 51.2, 57.0],
                ..Movement::default()
            },
        };
        let mut out = Vec::new();
        welcome.write(&mut out);
        let frame = one_frame(&out);
        let Ok(ServerMessage::Welcome(got)) = ServerMessage::read(&frame) else {
            panic!("not a welcome");
        };
        assert_eq!(got, welcome);
        assert_eq!(
            ClientMessage::read(&frame),
            Err(Error::Unexpected(Kind::Welcome as u8))
        );
    }
}
