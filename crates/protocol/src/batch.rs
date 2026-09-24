use crate::frame::{Kind, begin_frame};
use crate::message::{read_name, write_name};
use crate::reader::Reader;
use crate::{Appearance, Error, Movement};

const APPEAR: u8 = 1;
const VANISH: u8 = 2;
const MOVE: u8 = 3;
const CORRECT: u8 = 4;

/// One piece of a tick's news for one client. Entity ids are never reused.
#[derive(Clone, Debug, PartialEq)]
pub enum Record<'a> {
    /// An entity came into view: who, what it looks like, and how it moves.
    Appear {
        id: u32,
        name: &'a str,
        appearance: Appearance,
        movement: Movement,
    },
    /// An entity left view, or the world.
    Vanish { id: u32 },
    /// An entity in view moved.
    Move { id: u32, movement: Movement },
    /// The server refused this client's claim: its mover stands here, and its claims count again
    /// once they acknowledge `seq`.
    Correct { seq: u32, movement: Movement },
}

/// One tick's news for one client: its records, read one by one as the batch is iterated.
pub struct Batch<'a> {
    pub tick: u32,
    records: Reader<'a>,
}

impl<'a> Batch<'a> {
    pub(crate) fn read(mut r: Reader<'a>) -> Result<Self, Error> {
        Ok(Self {
            tick: r.u32()?,
            records: r,
        })
    }

    fn record(&mut self) -> Result<Record<'a>, Error> {
        let r = &mut self.records;
        Ok(match r.u8()? {
            APPEAR => Record::Appear {
                id: r.u32()?,
                name: read_name(r)?,
                appearance: Appearance::read(r)?,
                movement: Movement::read(r)?,
            },
            VANISH => Record::Vanish { id: r.u32()? },
            MOVE => Record::Move {
                id: r.u32()?,
                movement: Movement::read(r)?,
            },
            CORRECT => Record::Correct {
                seq: r.u32()?,
                movement: Movement::read(r)?,
            },
            other => return Err(Error::UnknownRecord(other)),
        })
    }
}

impl<'a> Iterator for Batch<'a> {
    type Item = Result<Record<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.records.is_empty() {
            return None;
        }
        let record = self.record();
        if record.is_err() {
            self.records = Reader::new(&[]);
        }
        Some(record)
    }
}

/// Opens tick `tick`'s batch at the end of `out`; append its records, then close it with
/// [`crate::finish_frame`] and the returned start.
pub fn begin_batch(out: &mut Vec<u8>, tick: u32) -> usize {
    let start = begin_frame(out, Kind::Batch);
    out.extend_from_slice(&tick.to_le_bytes());
    start
}

pub fn write_appear(
    out: &mut Vec<u8>,
    id: u32,
    name: &str,
    appearance: &Appearance,
    movement: &Movement,
) {
    out.push(APPEAR);
    out.extend_from_slice(&id.to_le_bytes());
    write_name(out, name);
    appearance.write(out);
    movement.write(out);
}

pub fn write_vanish(out: &mut Vec<u8>, id: u32) {
    out.push(VANISH);
    out.extend_from_slice(&id.to_le_bytes());
}

pub fn write_move(out: &mut Vec<u8>, id: u32, movement: &Movement) {
    out.push(MOVE);
    out.extend_from_slice(&id.to_le_bytes());
    movement.write(out);
}

pub fn write_correct(out: &mut Vec<u8>, seq: u32, movement: &Movement) {
    out.push(CORRECT);
    out.extend_from_slice(&seq.to_le_bytes());
    movement.write(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frames, ServerMessage, finish_frame, flags};

    fn batch_of(records: &[Record<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        let start = begin_batch(&mut out, 77);
        for r in records {
            match r {
                Record::Appear {
                    id,
                    name,
                    appearance,
                    movement,
                } => write_appear(&mut out, *id, name, appearance, movement),
                Record::Vanish { id } => write_vanish(&mut out, *id),
                Record::Move { id, movement } => write_move(&mut out, *id, movement),
                Record::Correct { seq, movement } => write_correct(&mut out, *seq, movement),
            }
        }
        finish_frame(&mut out, start);
        out
    }

    fn records_of(bytes: &[u8]) -> (u32, Vec<Result<Record<'_>, Error>>) {
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&bytes[crate::LEN_BYTES..])
        else {
            panic!("not a batch");
        };
        (batch.tick, batch.collect())
    }

    #[test]
    fn records_come_back_in_order() {
        let running = Movement {
            time: 5,
            flags: flags::FORWARD | flags::FALLING,
            pos: [1.0, 2.0, 3.0],
            ..Movement::default()
        };
        let records = [
            Record::Correct {
                seq: 3,
                movement: Movement::default(),
            },
            Record::Appear {
                id: 10,
                name: "Marshal Dughan",
                appearance: Appearance {
                    race: 1,
                    ..Appearance::default()
                },
                movement: running,
            },
            Record::Move {
                id: 11,
                movement: running,
            },
            Record::Vanish { id: 12 },
        ];
        let bytes = batch_of(&records);
        let mut frames = Frames::default();
        frames.extend(&bytes);
        assert!(frames.next_frame().expect("valid").is_some());
        let (tick, got) = records_of(&bytes);
        assert_eq!(tick, 77);
        let got: Vec<Record<'_>> = got.into_iter().map(|r| r.expect("valid")).collect();
        assert_eq!(got, records);
    }

    #[test]
    fn a_bad_record_ends_the_batch() {
        let mut bytes = batch_of(&[Record::Vanish { id: 1 }, Record::Vanish { id: 2 }]);
        let second = bytes.len() - 5;
        bytes[second] = 99;
        let (_, got) = records_of(&bytes);
        assert_eq!(
            got,
            vec![Ok(Record::Vanish { id: 1 }), Err(Error::UnknownRecord(99))]
        );
    }
}
