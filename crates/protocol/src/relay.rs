use crate::message::write_name;
use crate::reader::Reader;
use crate::{Angle, Appearance, Error, Jump, Movement, Pos, Wrapped, flags};

pub(crate) const MOVE_LEN: usize = Wrapped::ENCODED_LEN + 1;
const STATE_MAX: usize = 4 + Wrapped::ENCODED_LEN + 1 + 1 + 4 + 16;

/// How an entity moves, as a batch relays it: its [`Movement`] without the client's clock, the
/// position [`Wrapped`] and the angles in [`Angle`]s. The pitch travels only while swimming, the
/// fall time and the jump only while falling; otherwise they read as zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    pub flags: u32,
    pub pos: Wrapped,
    pub facing: Angle,
    pub pitch: Angle,
    pub fall_time: u32,
    pub jump: Jump,
}

impl State {
    pub fn of(m: &Movement) -> Self {
        let swimming = m.flags & flags::SWIMMING != 0;
        let falling = m.flags & flags::FALLING != 0;
        Self {
            flags: m.flags,
            pos: Pos::of(m.pos).wrapped(),
            facing: Angle::of(m.facing),
            pitch: if swimming {
                Angle::of(m.pitch)
            } else {
                Angle::default()
            },
            fall_time: if falling { m.fall_time } else { 0 },
            jump: if falling { m.jump } else { Jump::default() },
        }
    }

    fn encode(&self) -> Body<STATE_MAX> {
        let mut b = Body::default();
        b.put(&self.flags.to_le_bytes());
        b.put_pos(self.pos);
        b.put(&[self.facing.0]);
        if self.flags & flags::SWIMMING != 0 {
            b.put(&[self.pitch.0]);
        }
        if self.flags & flags::FALLING != 0 {
            b.put(&self.fall_time.to_le_bytes());
            let j = self.jump;
            for v in [j.z_speed, j.cos, j.sin, j.xy_speed] {
                b.put(&v.to_le_bytes());
            }
        }
        b
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let flags = r.u32()?;
        let pos = Wrapped::read(r)?;
        let facing = Angle(r.u8()?);
        let pitch = if flags & flags::SWIMMING != 0 {
            Angle(r.u8()?)
        } else {
            Angle::default()
        };
        let (fall_time, jump) = if flags & flags::FALLING != 0 {
            let fall_time = r.u32()?;
            let jump = Jump {
                z_speed: r.f32()?,
                cos: r.f32()?,
                sin: r.f32()?,
                xy_speed: r.f32()?,
            };
            (fall_time, jump)
        } else {
            (0, Jump::default())
        };
        Ok(Self {
            flags,
            pos,
            facing,
            pitch,
            fall_time,
            jump,
        })
    }

    fn moves_like(&self, other: &Self) -> bool {
        self.flags == other.flags && self.pitch == other.pitch && self.jump == other.jump
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Body<const N: usize> {
    bytes: [u8; N],
    len: u8,
}

impl<const N: usize> Default for Body<N> {
    fn default() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
}

impl<const N: usize> Body<N> {
    fn put(&mut self, b: &[u8]) {
        let at = usize::from(self.len);
        self.bytes[at..at + b.len()].copy_from_slice(b);
        self.len += b.len() as u8;
    }

    fn put_pos(&mut self, p: Wrapped) {
        for v in p.0 {
            self.put(&v.to_le_bytes());
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// One entity's movement encoded once a tick, for every batch that relays it to copy.
#[derive(Clone, Copy, Debug, Default)]
pub struct Relay {
    pos: Pos,
    state: State,
    pub(crate) moved: [u8; MOVE_LEN],
    pub(crate) stated: Body<STATE_MAX>,
}

/// What a batch relays differently from one [`Relay`] to the next.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed {
    pub pos: bool,
    pub facing: bool,
    /// The flags, the swim pitch or the jump.
    pub state: bool,
}

impl Relay {
    pub fn of(m: &Movement) -> Self {
        let state = State::of(m);
        let mut moved = [state.facing.0; MOVE_LEN];
        for (bytes, v) in moved.as_chunks_mut::<2>().0.iter_mut().zip(state.pos.0) {
            *bytes = v.to_le_bytes();
        }
        Self {
            pos: Pos::of(m.pos),
            state,
            moved,
            stated: state.encode(),
        }
    }

    pub fn changed_from(&self, before: &Self) -> Changed {
        Changed {
            pos: self.pos != before.pos,
            facing: self.state.facing != before.state.facing,
            state: !self.state.moves_like(&before.state),
        }
    }

    pub(crate) fn facing(&self) -> u8 {
        self.state.facing.0
    }
}

/// Who an entity is, as an appear introduces it: its id, name and look, encoded once.
#[derive(Clone, Debug, Default)]
pub struct Intro(pub(crate) Vec<u8>);

impl Intro {
    pub fn new(id: u32, name: &str, appearance: &Appearance) -> Self {
        let mut out = Vec::with_capacity(4 + 1 + name.len() + Appearance::ENCODED_LEN);
        out.extend_from_slice(&id.to_le_bytes());
        write_name(&mut out, name);
        appearance.write(&mut out);
        Self(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(flags: u32, pos: [f32; 3], facing: f32) -> Movement {
        Movement {
            time: 5000,
            flags,
            pos,
            facing,
            pitch: 0.3,
            fall_time: 120,
            jump: Jump {
                z_speed: -7.955_547,
                cos: 0.6,
                sin: 0.8,
                xy_speed: 7.0,
            },
        }
    }

    #[test]
    fn a_state_carries_each_tail_only_with_its_flag_and_reads_back() {
        for (f, len) in [
            (flags::FORWARD, 11),
            (flags::SWIMMING | flags::FORWARD, 12),
            (flags::FALLING | flags::FORWARD, 31),
        ] {
            let m = at(f, [-9439.1, 51.2, 57.25], 1.5);
            let state = State::of(&m);
            let body = state.encode();
            assert_eq!(body.bytes().len(), len, "flags {f:#x}");
            let mut r = Reader::new(body.bytes());
            assert_eq!(State::read(&mut r), Ok(state));
            assert!(r.is_empty());
        }
        let standing = State::of(&at(0, [1.0, 2.0, 3.0], 0.0));
        assert_eq!((standing.pitch, standing.fall_time), (Angle(0), 0));
    }

    #[test]
    fn a_change_is_what_the_relayed_steps_and_state_show() {
        let before = Relay::of(&at(flags::FORWARD, [10.0, 20.0, 5.0], 1.0));
        let changes = |m: Movement| Relay::of(&m).changed_from(&before);
        let nudged = at(flags::FORWARD, [10.001, 20.0, 5.0], 1.001);
        assert_eq!(changes(nudged), Changed::default());
        let stepped = at(flags::FORWARD, [10.01, 20.0, 5.0], 1.0);
        assert_eq!(
            changes(stepped),
            Changed {
                pos: true,
                ..Changed::default()
            }
        );
        let turned = at(flags::FORWARD, [10.0, 20.0, 5.0], 1.1);
        assert!(changes(turned).facing && !changes(turned).pos);
        let stopped = at(0, [10.0, 20.0, 5.0], 1.0);
        assert!(changes(stopped).state);
        let falling = Relay::of(&at(flags::FALLING, [10.0, 20.0, 5.0], 1.0));
        let later = Movement {
            fall_time: 400,
            ..at(flags::FALLING, [10.0, 20.0, 5.0], 1.0)
        };
        assert!(!Relay::of(&later).changed_from(&falling).state);
    }
}
