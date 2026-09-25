use std::f32::consts::TAU;

use crate::Error;
use crate::reader::Reader;

pub const STEPS_PER_YD: [f32; 3] = [128.0, 128.0, 32.0];
const WRAPPED_STEPS_EITHER_WAY: f32 = 32_768.0;

/// A world position rounded to whole steps of [`STEPS_PER_YD`], the precision a batch relays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pos(pub [i32; 3]);

impl Pos {
    /// The nearest step to a position in yards.
    pub fn of(yd: [f32; 3]) -> Self {
        Self(std::array::from_fn(|a| {
            (yd[a] * STEPS_PER_YD[a]).round() as i32
        }))
    }

    pub fn yards(self) -> [f32; 3] {
        std::array::from_fn(|a| self.0[a] as f32 / STEPS_PER_YD[a])
    }

    pub fn wrapped(self) -> Wrapped {
        Wrapped(self.0.map(|v| v as u16))
    }
}

/// A [`Pos`] as a batch carries it: the low 16 bits of each axis. They name one position in every
/// 512 yd across and 2,048 yd up or down, so a receiver reads them against a point it knows is
/// within half that of the entity, such as where the server holds its own mover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Wrapped(pub [u16; 3]);

impl Wrapped {
    pub const ENCODED_LEN: usize = 6;
    /// Yards an axis within which the bits read back as the position they were taken from: from
    /// this far below the point they are read against to short of this far above it.
    pub const REACH_YD: [f32; 3] = [
        WRAPPED_STEPS_EITHER_WAY / STEPS_PER_YD[0],
        WRAPPED_STEPS_EITHER_WAY / STEPS_PER_YD[1],
        WRAPPED_STEPS_EITHER_WAY / STEPS_PER_YD[2],
    ];

    /// The position these bits stand for that lies nearest `reference`, a point in yards.
    pub fn around(self, reference: [f32; 3]) -> Pos {
        let near = Pos::of(reference);
        Pos(std::array::from_fn(|a| {
            let offset = self.0[a].wrapping_sub(near.0[a] as u16) as i16;
            near.0[a].wrapping_add(i32::from(offset))
        }))
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Self([r.u16()?, r.u16()?, r.u16()?]))
    }
}

/// An angle in steps of 1/256 of a turn, as a batch carries facings and pitches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Angle(pub u8);

impl Angle {
    pub fn of(radians: f32) -> Self {
        Self(((radians / TAU * 256.0).round() as i32).rem_euclid(256) as u8)
    }

    /// The angle in radians, from 0 to a full turn.
    pub fn radians(self) -> f32 {
        f32::from(self.0) * TAU / 256.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn half_steps_off(p: Pos, yd: [f32; 3]) -> f32 {
        let back = p.yards();
        (0..3)
            .map(|a| (back[a] - yd[a]).abs() * STEPS_PER_YD[a])
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_position_rounds_to_the_nearest_step_anywhere_on_a_map() {
        for yd in [
            [0.0, 0.0, 0.0],
            [-9439.1, 51.2, 57.25],
            [17_066.0, -17_066.0, -512.3],
            [3.004, -3.004, 1.51],
        ] {
            assert!(half_steps_off(Pos::of(yd), yd) <= 0.5 + 1e-3, "{yd:?}");
        }
    }

    #[test]
    fn wrapped_bits_come_back_exactly_within_the_window_and_not_beyond() {
        let reference = [-9439.1, 51.2, 57.0];
        for (dx, dy, dz) in [
            (0.0, 0.0, 0.0),
            (101.0, -101.0, 40.0),
            (-255.9, 255.9, -1023.0),
            (0.004, 180.0, 900.0),
        ] {
            let p = Pos::of([reference[0] + dx, reference[1] + dy, reference[2] + dz]);
            assert_eq!(p.wrapped().around(reference), p, "{dx} {dy} {dz}");
        }
        let beyond = Pos::of([reference[0] + 256.5, reference[1], reference[2]]);
        let read = beyond.wrapped().around(reference);
        assert_eq!(read.0[0], beyond.0[0] - (1 << 16), "a whole window away");
        let near = Pos::of(reference);
        for (a, (reach_yd, steps)) in Wrapped::REACH_YD.iter().zip(STEPS_PER_YD).enumerate() {
            let reach = (reach_yd * steps) as i32;
            for (offset, back) in [
                (reach - 1, true),
                (reach, false),
                (-reach, true),
                (-reach - 1, false),
            ] {
                let mut p = near;
                p.0[a] += offset;
                let read = p.wrapped().around(reference);
                assert_eq!(read == p, back, "axis {a}, {offset} steps off");
            }
        }
    }

    #[test]
    fn angles_wrap_and_round_to_a_256th_of_a_turn() {
        assert_eq!(Angle::of(0.0), Angle(0));
        assert_eq!(Angle::of(TAU), Angle(0));
        assert_eq!(Angle::of(-TAU / 4.0), Angle(192));
        assert_eq!(Angle::of(3.0 * TAU + TAU / 2.0), Angle(128));
        for r in [0.1_f32, 1.0, 2.5, 6.2] {
            let off = (Angle::of(r).radians() - r).abs();
            assert!(off <= TAU / 512.0 + 1e-6, "{r}: {off}");
        }
    }
}
