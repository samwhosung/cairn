use crate::{Jump, Movement, flags};

/// How long a client with any movement flag set goes without a claim before it claims anyway,
/// milliseconds.
pub const HEARTBEAT_MS: u32 = 500;

const IN_MOTION: u32 = flags::ANY_MOVE | flags::FALLING | flags::FALLING_FAR | flags::SWIMMING;

/// When a client's movement goes out as claims, frame by frame, as the 1.12 client sends it: one
/// claim for each of forward/back, strafe, turn, walk and swim that changes, where the direction
/// keys go quiet mid-air unless a standing jump's one steer changes the arc; one for a jump's
/// launch and one for any landing; one for each frame the facing moves off the turn keys; a
/// heartbeat once [`HEARTBEAT_MS`] pass without a claim while any flag is set; and one when a body
/// at rest is no longer where it last claimed to be.
#[derive(Clone, Debug)]
pub struct Cadence {
    flags: u32,
    facing: f32,
    fall_time: u32,
    jump: Jump,
    sent_at: u32,
    sent_pos: [f32; 3],
    report: bool,
}

impl Cadence {
    /// A client standing still where the server placed it.
    pub fn new(spawn: &Movement) -> Self {
        Self {
            flags: 0,
            facing: spawn.facing,
            fall_time: 0,
            jump: Jump::default(),
            sent_at: spawn.time,
            sent_pos: spawn.pos,
            report: false,
        }
    }

    /// The next frame claims whatever it holds, even if nothing else would: after a correction,
    /// to acknowledge it.
    pub fn report_now(&mut self) {
        self.report = true;
    }

    /// How many claims this frame's movement makes, each carrying all of it.
    pub fn claims(&mut self, m: &Movement) -> usize {
        let (was, live) = (self.flags, m.flags);
        let (added, changed) = (live & !was, live ^ was);
        let (airborne, was_airborne) = (live & flags::FALLING != 0, was & flags::FALLING != 0);
        let launched =
            airborne && m.jump.z_speed < 0.0 && (!was_airborne || m.fall_time < self.fall_time);
        let steered = airborne && was_airborne && !launched && m.jump != self.jump;
        let heard = |axis: u32| {
            if airborne {
                steered && added & axis != 0
            } else {
                changed & axis != 0
            }
        };
        let modes = [flags::SWIMMING, flags::WALK_MODE, flags::TURNING];
        let directions = [
            flags::FORWARD | flags::BACKWARD,
            flags::STRAFE_LEFT | flags::STRAFE_RIGHT,
        ];
        let turning = (live | was) & flags::TURNING != 0;
        let mut n = usize::from(launched || (was_airborne && !airborne))
            + modes.iter().filter(|&&axis| changed & axis != 0).count()
            + directions.iter().filter(|&&axis| heard(axis)).count()
            + usize::from(!turning && m.facing.to_bits() != self.facing.to_bits());
        if n == 0 {
            let heartbeat = live != 0 && m.time.saturating_sub(self.sent_at) >= HEARTBEAT_MS;
            let drifted =
                live & IN_MOTION == 0 && m.pos.map(f32::to_bits) != self.sent_pos.map(f32::to_bits);
            n = usize::from(heartbeat || drifted || self.report);
        }
        if n > 0 {
            self.sent_at = m.time;
            self.sent_pos = m.pos;
            self.report = false;
        }
        self.flags = live;
        self.facing = m.facing;
        self.fall_time = m.fall_time;
        self.jump = m.jump;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: u32 = flags::FORWARD;

    fn at(time: u32, flags: u32, x: f32) -> Movement {
        Movement {
            time,
            flags,
            pos: [x, 0.0, 0.0],
            ..Movement::default()
        }
    }

    fn leaping(time: u32, flags: u32, fall_time: u32, xy_speed: f32) -> Movement {
        Movement {
            fall_time,
            jump: Jump {
                z_speed: -7.955_547,
                cos: 1.0,
                sin: 0.0,
                xy_speed,
            },
            ..at(time, flags | flags::FALLING, 0.0)
        }
    }

    fn sent(frames: &[Movement]) -> Vec<(u32, usize)> {
        let mut c = Cadence::new(&Movement::default());
        frames
            .iter()
            .map(|m| (m.time, c.claims(m)))
            .filter(|&(_, n)| n > 0)
            .collect()
    }

    #[test]
    fn a_run_claims_its_start_a_heartbeat_each_half_second_and_its_stop() {
        let frames: Vec<Movement> = (0..=60u32)
            .map(|i| i * 50)
            .map(|t| match t {
                0..1000 => at(t, 0, 0.0),
                1000..2600 => at(t, RUN, (t - 1000) as f32 / 100.0),
                _ => at(t, 0, 16.0),
            })
            .collect();
        assert_eq!(
            sent(&frames),
            [(1000, 1), (1500, 1), (2000, 1), (2500, 1), (2600, 1)]
        );
    }

    #[test]
    fn a_jump_claims_its_launch_and_landing_and_the_keys_mid_air_only_after_a_steer() {
        let mut c = Cadence::new(&Movement::default());
        assert_eq!(c.claims(&leaping(0, RUN, 0, 7.0)), 1, "the launch");
        assert_eq!(
            c.claims(&leaping(50, 0, 50, 7.0)),
            0,
            "a key let go mid-air"
        );
        assert_eq!(
            c.claims(&leaping(100, RUN, 100, 7.0)),
            0,
            "and pressed again"
        );
        assert_eq!(c.claims(&at(800, RUN, 5.0)), 1, "the landing");
        let mut c = Cadence::new(&Movement::default());
        c.claims(&leaping(0, 0, 0, 0.0));
        assert_eq!(
            c.claims(&leaping(50, RUN, 50, 2.5)),
            1,
            "a standing jump's steer"
        );
        assert_eq!(
            c.claims(&at(850, 0, 5.0)),
            2,
            "the landing and the key let go"
        );
        c.claims(&at(875, RUN, 5.0));
        let falling = |t: u32, fall_time: u32| Movement {
            fall_time,
            ..at(t, RUN | flags::FALLING, 5.0)
        };
        assert_eq!(
            c.claims(&falling(900, 0)),
            0,
            "a fall without a jump opens silently"
        );
        assert_eq!(c.claims(&falling(925, 25)), 0);
        assert_eq!(
            c.claims(&leaping(950, RUN, 0, 7.0)),
            1,
            "a landing that jumps again in its frame"
        );
    }

    #[test]
    fn a_mouse_turn_claims_every_frame_and_a_key_turn_only_its_ends() {
        let turned = |t: u32, flags: u32, facing: f32| Movement {
            facing,
            ..at(t, flags, 0.0)
        };
        let frames: Vec<Movement> = (0..10u32)
            .map(|i| turned(i * 50, 0, i as f32 * 0.1))
            .collect();
        assert_eq!(sent(&frames).len(), 9, "every frame the facing moved");
        let frames = [
            turned(0, flags::TURN_LEFT, 0.1),
            turned(50, flags::TURN_LEFT, 0.2),
            turned(500, flags::TURN_LEFT, 1.0),
            turned(550, 0, 1.1),
            turned(600, 0, 1.1),
        ];
        assert_eq!(sent(&frames), [(0, 1), (500, 1), (550, 1)]);
    }

    #[test]
    fn a_body_at_rest_claims_a_drift_and_a_correction_is_acknowledged_at_once() {
        let mut c = Cadence::new(&Movement::default());
        assert_eq!(c.claims(&at(50, 0, 0.0)), 0);
        assert_eq!(c.claims(&at(100, 0, 0.001)), 1, "settled a hair lower");
        assert_eq!(c.claims(&at(150, 0, 0.001)), 0);
        c.report_now();
        assert_eq!(c.claims(&at(200, 0, 0.001)), 1);
        assert_eq!(c.claims(&at(250, flags::WALK_MODE, 0.001)), 1, "walk mode");
        assert_eq!(c.claims(&at(700, flags::WALK_MODE, 0.001)), 0);
        assert_eq!(
            c.claims(&at(750, flags::WALK_MODE, 0.001)),
            1,
            "a heartbeat"
        );
    }
}
