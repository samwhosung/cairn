use crate::Error;
use crate::reader::Reader;

/// The movement flag word: the 1.12 client's bits, so a client's own word goes out unchanged.
pub mod flags {
    pub const FORWARD: u32 = 0x1;
    pub const BACKWARD: u32 = 0x2;
    pub const STRAFE_LEFT: u32 = 0x4;
    pub const STRAFE_RIGHT: u32 = 0x8;
    pub const TURN_LEFT: u32 = 0x10;
    pub const TURN_RIGHT: u32 = 0x20;
    pub const WALK_MODE: u32 = 0x100;
    pub const ROOT: u32 = 0x1000;
    /// Airborne; a movement with it carries the [`crate::Jump`] that launched the arc.
    pub const FALLING: u32 = 0x2000;
    pub const FALLING_FAR: u32 = 0x4000;
    /// Swimming; a movement with it carries the swim pitch.
    pub const SWIMMING: u32 = 0x20_0000;
    pub const ANY_MOVE: u32 = FORWARD | BACKWARD | STRAFE_LEFT | STRAFE_RIGHT;
    pub const TURNING: u32 = TURN_LEFT | TURN_RIGHT;
    /// Set while a body's pose changes of itself.
    pub const UNDER_WAY: u32 = ANY_MOVE | TURNING | FALLING;
}

/// The launch an airborne arc replays: the vertical speed, down positive as the client sends it,
/// and the horizontal velocity frozen at take-off as a direction and a speed, yards per second.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Jump {
    pub z_speed: f32,
    pub cos: f32,
    pub sin: f32,
    pub xy_speed: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Movement {
    /// The reporting client's clock, milliseconds.
    pub time: u32,
    pub flags: u32,
    /// World coordinates in yards: x north, y west, z up.
    pub pos: [f32; 3],
    /// Radians counterclockwise from north.
    pub facing: f32,
    /// Sent only while [`flags::SWIMMING`]; read as 0 otherwise.
    pub pitch: f32,
    /// Milliseconds airborne.
    pub fall_time: u32,
    /// Sent only while [`flags::FALLING`]; read as zeros otherwise.
    pub jump: Jump,
}

impl Movement {
    pub fn encoded_len(&self) -> usize {
        let mut n = 28;
        if self.flags & flags::SWIMMING != 0 {
            n += 4;
        }
        if self.flags & flags::FALLING != 0 {
            n += 16;
        }
        n
    }

    pub fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.time.to_le_bytes());
        out.extend_from_slice(&self.flags.to_le_bytes());
        for v in [self.pos[0], self.pos[1], self.pos[2], self.facing] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if self.flags & flags::SWIMMING != 0 {
            out.extend_from_slice(&self.pitch.to_le_bytes());
        }
        out.extend_from_slice(&self.fall_time.to_le_bytes());
        if self.flags & flags::FALLING != 0 {
            let j = self.jump;
            for v in [j.z_speed, j.cos, j.sin, j.xy_speed] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        let time = r.u32()?;
        let flags = r.u32()?;
        let pos = [r.f32()?, r.f32()?, r.f32()?];
        let facing = r.f32()?;
        let pitch = if flags & flags::SWIMMING != 0 {
            r.f32()?
        } else {
            0.0
        };
        let fall_time = r.u32()?;
        let jump = if flags & flags::FALLING != 0 {
            Jump {
                z_speed: r.f32()?,
                cos: r.f32()?,
                sin: r.f32()?,
                xy_speed: r.f32()?,
            }
        } else {
            Jump::default()
        };
        Ok(Self {
            time,
            flags,
            pos,
            facing,
            pitch,
            fall_time,
            jump,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(flags: u32) -> Movement {
        Movement {
            time: 123_456,
            flags,
            pos: [-9439.1, 51.2, 57.25],
            facing: 1.5,
            pitch: if flags & flags::SWIMMING != 0 {
                -0.25
            } else {
                0.0
            },
            fall_time: 480,
            jump: if flags & flags::FALLING != 0 {
                Jump {
                    z_speed: -7.955_547,
                    cos: 0.6,
                    sin: 0.8,
                    xy_speed: 7.0,
                }
            } else {
                Jump::default()
            },
        }
    }

    #[test]
    fn each_tail_travels_with_its_flag_and_round_trips() {
        for (f, len) in [
            (flags::FORWARD, 28),
            (flags::SWIMMING | flags::FORWARD, 32),
            (flags::FALLING | flags::FORWARD, 44),
            (flags::FALLING | flags::SWIMMING, 48),
        ] {
            let m = sample(f);
            let mut out = Vec::new();
            m.write(&mut out);
            assert_eq!(out.len(), len, "flags {f:#x}");
            assert_eq!(m.encoded_len(), len);
            let mut r = Reader::new(&out);
            assert_eq!(Movement::read(&mut r), Ok(m));
            assert!(r.is_empty());
        }
    }

    #[test]
    fn a_cut_movement_is_truncated() {
        let mut out = Vec::new();
        sample(flags::FALLING).write(&mut out);
        out.pop();
        assert_eq!(
            Movement::read(&mut Reader::new(&out)),
            Err(Error::Truncated)
        );
    }
}
