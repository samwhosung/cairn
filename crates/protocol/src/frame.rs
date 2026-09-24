use crate::Error;

/// A frame's length prefix, which counts the bytes after it.
pub const LEN_BYTES: usize = 4;
/// The longest frame either side accepts.
pub const MAX_FRAME: usize = 16 << 20;
/// Spent bytes are dropped from the front of [`Frames`]' buffer once there are this many.
const COMPACT_AT: usize = 1 << 16;

/// What a frame carries, as its first byte says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Hello = 1,
    Claim = 2,
    Welcome = 3,
    Batch = 4,
}

impl Kind {
    pub(crate) fn of(byte: u8) -> Result<Self, Error> {
        match byte {
            1 => Ok(Self::Hello),
            2 => Ok(Self::Claim),
            3 => Ok(Self::Welcome),
            4 => Ok(Self::Batch),
            k => Err(Error::UnknownKind(k)),
        }
    }
}

/// Opens a frame of `kind` at the end of `out` and returns where it starts, for
/// [`finish_frame`] to write its length once the message is in.
pub fn begin_frame(out: &mut Vec<u8>, kind: Kind) -> usize {
    let start = out.len();
    out.extend_from_slice(&[0; LEN_BYTES]);
    out.push(kind as u8);
    start
}

pub fn finish_frame(out: &mut [u8], start: usize) {
    let len = (out.len() - start - LEN_BYTES) as u32;
    out[start..start + LEN_BYTES].copy_from_slice(&len.to_le_bytes());
}

/// Cuts a byte stream into frames: [`Frames::extend`] with what arrived, then take the whole
/// frames from [`Frames::next_frame`].
#[derive(Default)]
pub struct Frames {
    buf: Vec<u8>,
    start: usize,
}

impl Frames {
    pub fn extend(&mut self, bytes: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start >= COMPACT_AT {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole frame, kind byte first, or `None` until more bytes arrive.
    pub fn next_frame(&mut self) -> Result<Option<&[u8]>, Error> {
        let rest = &self.buf[self.start..];
        let Some(prefix) = rest.first_chunk::<LEN_BYTES>() else {
            return Ok(None);
        };
        let len = u32::from_le_bytes(*prefix) as usize;
        if len == 0 || len > MAX_FRAME {
            return Err(Error::FrameLength(len));
        }
        if rest.len() < LEN_BYTES + len {
            return Ok(None);
        }
        let body = self.start + LEN_BYTES;
        self.start = body + len;
        Ok(Some(&self.buf[body..body + len]))
    }

    /// Bytes held that no whole frame has taken yet.
    pub fn pending(&self) -> usize {
        self.buf.len() - self.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(kind: Kind, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let start = begin_frame(&mut out, kind);
        out.extend_from_slice(body);
        finish_frame(&mut out, start);
        out
    }

    #[test]
    fn frames_survive_any_split_of_the_stream() {
        let mut stream = frame(Kind::Claim, &[1, 2, 3]);
        stream.extend(frame(Kind::Batch, &[9; 300]));
        stream.extend(frame(Kind::Hello, &[]));
        for step in [1, 2, 3, 7, 64, stream.len()] {
            let mut frames = Frames::default();
            let mut got = Vec::new();
            for piece in stream.chunks(step) {
                frames.extend(piece);
                while let Some(f) = frames.next_frame().expect("well formed") {
                    got.push(f.to_vec());
                }
            }
            assert_eq!(got.len(), 3, "split every {step}");
            assert_eq!(got[0], [Kind::Claim as u8, 1, 2, 3]);
            assert_eq!(got[1].len(), 301);
            assert_eq!(got[2], [Kind::Hello as u8]);
            assert_eq!(frames.pending(), 0);
        }
    }

    #[test]
    fn empty_and_oversized_lengths_fail() {
        let mut frames = Frames::default();
        frames.extend(&0u32.to_le_bytes());
        assert_eq!(frames.next_frame(), Err(Error::FrameLength(0)));
        let mut frames = Frames::default();
        let huge = MAX_FRAME as u32 + 1;
        frames.extend(&huge.to_le_bytes());
        assert_eq!(frames.next_frame(), Err(Error::FrameLength(MAX_FRAME + 1)));
    }
}
