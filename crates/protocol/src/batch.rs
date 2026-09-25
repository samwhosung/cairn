use std::fmt;

use crate::frame::{Kind, begin_frame};
use crate::message::read_name;
use crate::reader::Reader;
use crate::{Angle, Appearance, Error, Intro, Movement, Relay, State, Wrapped};

const MOVE: u16 = 0;
const TURN: u16 = 1;
const STATE: u16 = 2;
const OTHER: u16 = 3;
const KIND_SHIFT: u16 = 14;

const APPEAR: u8 = 1;
const VANISH: u8 = 2;
const CORRECT: u8 = 3;
const GRANTED: u8 = 4;
const PLACE: u8 = 5;
const GAME: u8 = 6;
const SHOW: u8 = 7;
const SHOW_OWN: u8 = 8;

const PLAY: u8 = 0;
const HOLD: u8 = 1;
const LET_GO: u8 = 2;

/// How many slots a client's view has: one for each entity in it.
pub const SLOTS: u16 = 1 << KIND_SHIFT;

/// One piece of a tick's news for one client. Each record opens with a little-endian `u16`. A
/// move, a turn or a state has its kind in the top two bits; every other record shares the fourth
/// kind and names itself in the next byte. The low 14 bits are a slot, the client's own number for
/// an entity in its view, given by the appear that brings the entity in and free again once it
/// vanishes; a correct's, a grant's, a place's and an own show's are 0.
#[derive(Clone, Debug, PartialEq)]
pub enum Record<'a> {
    /// An entity came into view and holds `slot` from now on.
    Appear {
        slot: u16,
        id: u32,
        name: &'a str,
        appearance: Appearance,
        state: State,
    },
    /// The entity in `slot` left view, or the world, and the slot is free.
    Vanish { slot: u16 },
    /// The entity in `slot` stands and faces here now; its other state is as last relayed.
    Move {
        slot: u16,
        pos: Wrapped,
        facing: Angle,
    },
    /// The entity in `slot` faces this way now, where it stood.
    Turn { slot: u16, facing: Angle },
    /// The entity in `slot` changed how it moves.
    State { slot: u16, state: State },
    /// The server refused this client's claim or teleport, for `why`: its mover stands here, and
    /// its claims count again once they acknowledge `seq`.
    Correct {
        seq: u32,
        why: Why,
        movement: Movement,
    },
    /// The server took every teleport of this client's up to `movement`'s clock that no correction
    /// answered, and holds its mover here at the end of the tick that took them.
    Granted { movement: Movement },
    /// The game put this client's mover here, rooted or free: a rooted mover may turn and fall but
    /// not move over the ground. Its claims count again once they acknowledge `seq`.
    Place {
        seq: u32,
        rooted: bool,
        movement: Movement,
    },
    /// The state of the entity in `slot` that the game running on the server shows, as that game
    /// encodes it; a client that does not know the game passes over it.
    Game { slot: u16, state: &'a [u8] },
    /// What the game has the entity in `slot` show, or with no slot this client's own mover.
    Show { slot: Option<u16>, show: Show },
}

/// What a game has a body show, in the install's `AnimationData.dbc` ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    /// The animation plays once, from its start.
    Play(u16),
    /// The body holds this pose until it is told another, or lets it go with `None`.
    Hold(Option<u16>),
}

impl Show {
    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let (how, anim) = (r.u8()?, r.u16()?);
        match how {
            PLAY => Ok(Self::Play(anim)),
            HOLD => Ok(Self::Hold(Some(anim))),
            LET_GO => Ok(Self::Hold(None)),
            other => Err(Error::UnknownShow(other)),
        }
    }
}

/// Why the server refused a claim or a teleport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Why {
    Malformed,
    Clock,
    Speed,
    Climb,
    Fall,
    Launch,
    Teleport,
}

impl Why {
    pub const ALL: [Self; 7] = [
        Self::Malformed,
        Self::Clock,
        Self::Speed,
        Self::Climb,
        Self::Fall,
        Self::Launch,
        Self::Teleport,
    ];

    fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let byte = r.u8()?;
        Self::ALL
            .get(usize::from(byte))
            .copied()
            .ok_or(Error::UnknownWhy(byte))
    }
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "its movement did not parse as one",
            Self::Clock => "its clock ran backwards or too far ahead",
            Self::Speed => "it went further than its speed allows",
            Self::Climb => "it rose higher than it can climb",
            Self::Fall => "it fell faster than anything falls",
            Self::Launch => "it jumped faster than it can run",
            Self::Teleport => "this server lets only its host teleport",
        })
    }
}

/// One tick's news for one client: its records, read one by one as the batch is iterated. The
/// tick is the time of every record in it. Every entity's position in it reads right around where
/// the server holds the client's own mover, which a correction, a grant or a placement, coming
/// first, names.
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
        let head = r.u16()?;
        let slot = head & (SLOTS - 1);
        Ok(match head >> KIND_SHIFT {
            MOVE => Record::Move {
                slot,
                pos: Wrapped::read(r)?,
                facing: Angle(r.u8()?),
            },
            TURN => Record::Turn {
                slot,
                facing: Angle(r.u8()?),
            },
            STATE => Record::State {
                slot,
                state: State::read(r)?,
            },
            _ => match r.u8()? {
                APPEAR => Record::Appear {
                    slot,
                    id: r.u32()?,
                    name: read_name(r)?,
                    appearance: Appearance::read(r)?,
                    state: State::read(r)?,
                },
                VANISH => Record::Vanish { slot },
                CORRECT => Record::Correct {
                    seq: r.u32()?,
                    why: Why::read(r)?,
                    movement: Movement::read(r)?,
                },
                GRANTED => Record::Granted {
                    movement: Movement::read(r)?,
                },
                PLACE => Record::Place {
                    seq: r.u32()?,
                    rooted: r.u8()? != 0,
                    movement: Movement::read(r)?,
                },
                GAME => {
                    let len = usize::from(r.u16()?);
                    Record::Game {
                        slot,
                        state: r.bytes(len)?,
                    }
                }
                SHOW => Record::Show {
                    slot: Some(slot),
                    show: Show::read(r)?,
                },
                SHOW_OWN => Record::Show {
                    slot: None,
                    show: Show::read(r)?,
                },
                other => return Err(Error::UnknownRecord(other)),
            },
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

fn head(out: &mut Vec<u8>, kind: u16, slot: u16) {
    debug_assert!(slot < SLOTS);
    out.extend_from_slice(&(kind << KIND_SHIFT | slot).to_le_bytes());
}

/// Appends a record and returns how many of its bytes it copied from `intro` and `relay`.
pub fn write_appear(out: &mut Vec<u8>, slot: u16, intro: &Intro, relay: &Relay) -> usize {
    head(out, OTHER, slot);
    out.push(APPEAR);
    out.extend_from_slice(&intro.0);
    out.extend_from_slice(relay.stated.bytes());
    intro.0.len() + relay.stated.bytes().len()
}

pub fn write_vanish(out: &mut Vec<u8>, slot: u16) {
    head(out, OTHER, slot);
    out.push(VANISH);
}

/// Appends a record and returns how many of its bytes it copied from `relay`.
pub fn write_move(out: &mut Vec<u8>, slot: u16, relay: &Relay) -> usize {
    head(out, MOVE, slot);
    out.extend_from_slice(&relay.moved);
    relay.moved.len()
}

/// Appends a record and returns how many of its bytes it copied from `relay`.
pub fn write_turn(out: &mut Vec<u8>, slot: u16, relay: &Relay) -> usize {
    head(out, TURN, slot);
    out.push(relay.facing());
    1
}

/// Appends a record and returns how many of its bytes it copied from `relay`.
pub fn write_state(out: &mut Vec<u8>, slot: u16, relay: &Relay) -> usize {
    head(out, STATE, slot);
    out.extend_from_slice(relay.stated.bytes());
    relay.stated.bytes().len()
}

pub fn write_correct(out: &mut Vec<u8>, seq: u32, why: Why, movement: &Movement) {
    head(out, OTHER, 0);
    out.push(CORRECT);
    out.extend_from_slice(&seq.to_le_bytes());
    out.push(why as u8);
    movement.write(out);
}

pub fn write_granted(out: &mut Vec<u8>, movement: &Movement) {
    head(out, OTHER, 0);
    out.push(GRANTED);
    movement.write(out);
}

pub fn write_place(out: &mut Vec<u8>, seq: u32, rooted: bool, movement: &Movement) {
    head(out, OTHER, 0);
    out.push(PLACE);
    out.extend_from_slice(&seq.to_le_bytes());
    out.push(u8::from(rooted));
    movement.write(out);
}

/// Appends a record and returns how many of its bytes it copied from `state`.
///
/// # Panics
/// If `state` is longer than 65,535 bytes.
pub fn write_game(out: &mut Vec<u8>, slot: u16, state: &[u8]) -> usize {
    let len = u16::try_from(state.len()).expect("a game's state of one entity fits in 64 KiB");
    head(out, OTHER, slot);
    out.push(GAME);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(state);
    state.len()
}

/// Appends what the entity in `slot`, or with no slot the client's own mover, shows.
pub fn write_show(out: &mut Vec<u8>, slot: Option<u16>, show: Show) {
    head(out, OTHER, slot.unwrap_or(0));
    out.push(if slot.is_some() { SHOW } else { SHOW_OWN });
    let (how, anim) = match show {
        Show::Play(anim) => (PLAY, anim),
        Show::Hold(Some(anim)) => (HOLD, anim),
        Show::Hold(None) => (LET_GO, 0),
    };
    out.push(how);
    out.extend_from_slice(&anim.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Frames, Jump, ServerMessage, finish_frame, flags};

    fn records_of(bytes: &[u8]) -> (u32, Vec<Result<Record<'_>, Error>>) {
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&bytes[crate::LEN_BYTES..])
        else {
            panic!("not a batch");
        };
        (batch.tick, batch.collect())
    }

    fn running(pos: [f32; 3], facing: f32) -> Movement {
        Movement {
            time: 5,
            flags: flags::FORWARD,
            pos,
            facing,
            ..Movement::default()
        }
    }

    #[test]
    fn records_come_back_in_order_at_their_sizes() {
        let leaping = Movement {
            flags: flags::FORWARD | flags::FALLING,
            fall_time: 40,
            jump: Jump {
                z_speed: -7.955_547,
                cos: 0.6,
                sin: 0.8,
                xy_speed: 7.0,
            },
            ..running([1.0, 2.0, 3.0], 0.5)
        };
        let here = running([-9439.1, 51.2, 57.25], 2.0);
        let (walker, jumper) = (Relay::of(&here), Relay::of(&leaping));
        let look = Appearance {
            race: 1,
            ..Appearance::default()
        };
        let mut out = Vec::new();
        let start = begin_batch(&mut out, 77);
        let mut ends = vec![out.len()];
        write_correct(&mut out, 3, Why::Teleport, &Movement::default());
        ends.push(out.len());
        write_appear(
            &mut out,
            0,
            &Intro::new(10, "Marshal Dughan", &look),
            &jumper,
        );
        ends.push(out.len());
        write_move(&mut out, 1, &walker);
        ends.push(out.len());
        write_turn(&mut out, SLOTS - 1, &walker);
        ends.push(out.len());
        write_state(&mut out, 2, &walker);
        ends.push(out.len());
        write_vanish(&mut out, 1);
        ends.push(out.len());
        finish_frame(&mut out, start);
        let sizes: Vec<usize> = ends.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(ends[0], 9, "the batch's own header");
        assert_eq!(sizes, [36, 2 + 1 + 4 + 15 + 47 + 31, 9, 3, 13, 3]);

        let mut frames = Frames::default();
        frames.extend(&out);
        assert!(frames.next_frame().expect("valid").is_some());
        let (tick, got) = records_of(&out);
        assert_eq!(tick, 77);
        let got: Vec<Record<'_>> = got.into_iter().map(|r| r.expect("valid")).collect();
        let state = State::of(&here);
        assert_eq!(
            got,
            [
                Record::Correct {
                    seq: 3,
                    why: Why::Teleport,
                    movement: Movement::default()
                },
                Record::Appear {
                    slot: 0,
                    id: 10,
                    name: "Marshal Dughan",
                    appearance: look,
                    state: State::of(&leaping),
                },
                Record::Move {
                    slot: 1,
                    pos: state.pos,
                    facing: state.facing
                },
                Record::Turn {
                    slot: SLOTS - 1,
                    facing: state.facing
                },
                Record::State { slot: 2, state },
                Record::Vanish { slot: 1 },
            ]
        );
    }

    #[test]
    fn a_grant_comes_back_with_the_movement_it_holds() {
        let landed = running([-9439.1, 51.2, 57.25], 1.0);
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 5);
        write_granted(&mut bytes, &landed);
        finish_frame(&mut bytes, start);
        let (_, got) = records_of(&bytes);
        assert_eq!(got, vec![Ok(Record::Granted { movement: landed })]);
    }

    #[test]
    fn a_place_and_a_game_state_come_back_and_the_state_is_passed_over_by_its_length() {
        let placed = running([-9439.1, 51.2, 57.25], 1.0);
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 5);
        write_place(&mut bytes, 4, true, &placed);
        let state = [7u8, 0, 0, 0, 1];
        assert_eq!(write_game(&mut bytes, 12, &state), 5);
        write_game(&mut bytes, 13, &[]);
        write_vanish(&mut bytes, 12);
        finish_frame(&mut bytes, start);
        let (_, got) = records_of(&bytes);
        let got: Vec<Record<'_>> = got.into_iter().map(|r| r.expect("valid")).collect();
        assert_eq!(
            got,
            [
                Record::Place {
                    seq: 4,
                    rooted: true,
                    movement: placed
                },
                Record::Game {
                    slot: 12,
                    state: &state
                },
                Record::Game {
                    slot: 13,
                    state: &[]
                },
                Record::Vanish { slot: 12 },
            ]
        );
        let mut cut = Vec::new();
        let start = begin_batch(&mut cut, 5);
        write_game(&mut cut, 1, &state);
        cut.pop();
        finish_frame(&mut cut, start);
        assert_eq!(records_of(&cut).1, vec![Err(Error::Truncated)]);
    }

    #[test]
    fn what_a_body_shows_comes_back_for_a_slot_and_for_the_clients_own_mover() {
        let shows = [
            (Some(3), Show::Play(16)),
            (Some(SLOTS - 1), Show::Hold(Some(6))),
            (Some(0), Show::Hold(None)),
            (None, Show::Play(9)),
            (None, Show::Hold(Some(6))),
            (None, Show::Hold(None)),
        ];
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 5);
        for (slot, show) in shows {
            write_show(&mut bytes, slot, show);
        }
        let unknown = bytes.len() + 3;
        write_show(&mut bytes, Some(1), Show::Play(1));
        bytes[unknown] = LET_GO + 1;
        finish_frame(&mut bytes, start);
        let (_, got) = records_of(&bytes);
        let mut want: Vec<Result<Record<'_>, Error>> = shows
            .into_iter()
            .map(|(slot, show)| Ok(Record::Show { slot, show }))
            .collect();
        want.push(Err(Error::UnknownShow(LET_GO + 1)));
        assert_eq!(got, want);
        let mut one = Vec::new();
        write_show(&mut one, Some(2), Show::Play(16));
        assert_eq!(one.len(), 6);
    }

    #[test]
    fn a_bad_record_ends_the_batch() {
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 1);
        write_vanish(&mut bytes, 1);
        write_vanish(&mut bytes, 2);
        finish_frame(&mut bytes, start);
        let last = bytes.len() - 1;
        bytes[last] = 99;
        let (_, got) = records_of(&bytes);
        assert_eq!(
            got,
            vec![
                Ok(Record::Vanish { slot: 1 }),
                Err(Error::UnknownRecord(99))
            ]
        );
    }

    #[test]
    fn every_reason_for_a_correction_round_trips_and_no_other_is_read() {
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 1);
        for (seq, why) in (0..).zip(Why::ALL) {
            write_correct(&mut bytes, seq, why, &Movement::default());
        }
        let unknown = bytes.len() + 7;
        write_correct(&mut bytes, 9, Why::Malformed, &Movement::default());
        bytes[unknown] = Why::ALL.len() as u8;
        finish_frame(&mut bytes, start);
        let (_, got) = records_of(&bytes);
        let whys: Vec<Result<Why, Error>> = got
            .into_iter()
            .map(|r| {
                r.map(|r| match r {
                    Record::Correct { why, .. } => why,
                    other => panic!("{other:?}"),
                })
            })
            .collect();
        let mut want: Vec<Result<Why, Error>> = Why::ALL.into_iter().map(Ok).collect();
        want.push(Err(Error::UnknownWhy(Why::ALL.len() as u8)));
        assert_eq!(whys, want);
    }

    #[test]
    fn a_cut_record_is_truncated() {
        let mut bytes = Vec::new();
        let start = begin_batch(&mut bytes, 1);
        write_move(&mut bytes, 4, &Relay::of(&running([1.0, 2.0, 3.0], 0.0)));
        bytes.pop();
        finish_frame(&mut bytes, start);
        let (_, got) = records_of(&bytes);
        assert_eq!(got, vec![Err(Error::Truncated)]);
    }
}
