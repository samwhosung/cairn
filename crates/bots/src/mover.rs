use std::f32::consts::TAU;

use protocol::{Jump, Movement, flags};

use crate::ground::Ground;
use crate::track::Track;

pub const HEARTBEAT_MS: u32 = 500;
const JUMP_SPEED: f32 = 7.955_547;
const GRAVITY: f32 = 19.291_105;
const TELEPORT_YD: f32 = 200.0;

#[derive(Clone, Copy)]
struct Air {
    launched_at: u32,
    launch_z: f32,
    xy_speed: f32,
    facing: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Told {
    Truth,
    Lie,
}

/// A client's movement stream as the 1.12 client sends it, frame by frame: a claim for each
/// change of movement flags (the direction keys go quiet mid-air), one for each frame the facing
/// changes off the turn keys, and a heartbeat when [`HEARTBEAT_MS`] has passed without either
/// while moving.
pub struct Mover {
    sent_flags: u32,
    facing: f32,
    last_send: u32,
    report_now: bool,
    last_ground_z: f32,
    air: Option<Air>,
    jumped_leg: Option<usize>,
    teleported: bool,
    pub ack: u32,
}

impl Mover {
    pub fn new(ground_z: f32, facing: f32) -> Self {
        Self {
            sent_flags: 0,
            facing: facing.rem_euclid(TAU),
            last_send: 0,
            report_now: false,
            last_ground_z: ground_z,
            air: None,
            jumped_leg: None,
            teleported: false,
            ack: 0,
        }
    }

    pub fn correct(&mut self, seq: u32) {
        self.ack = seq;
        self.report_now = true;
    }

    pub fn frame(
        &mut self,
        t: u32,
        track: &Track,
        ground: &Ground,
        out: &mut Vec<Movement>,
    ) -> Told {
        let pose = track.pose(t);
        let [x, y] = pose.place.xy;
        let ground_z = ground.height(x, y).unwrap_or(self.last_ground_z);
        self.last_ground_z = ground_z;
        let mut live = pose.flags;
        if let Some(jump) = pose.jump
            && t >= jump.at_ms
            && self.air.is_none()
            && self.jumped_leg != Some(pose.leg)
        {
            self.jumped_leg = Some(pose.leg);
            self.air = Some(Air {
                launched_at: jump.at_ms,
                launch_z: ground_z,
                xy_speed: jump.speed,
                facing: pose.place.facing,
            });
        }
        let mut movement = Movement {
            time: t,
            pos: [x, y, ground_z],
            facing: pose.place.facing.rem_euclid(TAU),
            ..Movement::default()
        };
        if let Some(air) = self.air {
            let secs = (t - air.launched_at) as f32 / 1000.0;
            let z = air.launch_z + JUMP_SPEED * secs - 0.5 * GRAVITY * secs * secs;
            movement.fall_time = t - air.launched_at;
            if secs > 0.2 && z <= ground_z {
                self.air = None;
            } else {
                live |= flags::FALLING;
                movement.pos[2] = z;
                movement.jump = Jump {
                    z_speed: -JUMP_SPEED,
                    cos: air.facing.cos(),
                    sin: air.facing.sin(),
                    xy_speed: air.xy_speed,
                };
            }
        }
        movement.flags = live;
        let changes = self.changes(live, movement.facing);
        let heartbeat = live != 0 && t.saturating_sub(self.last_send) >= HEARTBEAT_MS;
        let mut claims = changes + usize::from(changes == 0 && (heartbeat || self.report_now));
        let fast = track.fast_lie_xy(t);
        if let Some(lie) = fast {
            movement.pos[0] = lie[0];
            movement.pos[1] = lie[1];
        }
        let teleport = track.lie.is_some_and(|lie| {
            !self.teleported && t >= lie.teleport_at && live & flags::FORWARD != 0
        });
        if teleport {
            self.teleported = true;
            movement.pos[0] += TELEPORT_YD;
            claims = claims.max(1);
        }
        if claims > 0 {
            self.last_send = t;
            self.report_now = false;
        }
        out.extend(std::iter::repeat_n(movement, claims));
        self.sent_flags = live;
        self.facing = movement.facing;
        if claims > 0 && (fast.is_some() || teleport) {
            Told::Lie
        } else {
            Told::Truth
        }
    }

    /// The claim that releases a turn key carries the turn's last facing itself.
    fn changes(&self, live: u32, facing: f32) -> usize {
        let changed = live ^ self.sent_flags;
        let airborne = live & flags::FALLING != 0;
        let axes = [
            (flags::FALLING, true),
            (flags::WALK_MODE, true),
            (flags::FORWARD | flags::BACKWARD, !airborne),
            (flags::STRAFE_LEFT | flags::STRAFE_RIGHT, !airborne),
            (flags::TURNING, true),
        ];
        let flag_claims = axes
            .iter()
            .filter(|&&(axis, heard)| heard && changed & axis != 0)
            .count();
        let turning = (live | self.sent_flags) & flags::TURNING != 0;
        let turned = !turning && facing.to_bits() != self.facing.to_bits();
        flag_claims + usize::from(turned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{Leg, Lie, Motion, RUN};

    fn leg(start_ms: u32, end_ms: u32, motion: Motion) -> Leg {
        Leg {
            start_ms,
            end_ms,
            from: [0.0, 0.0],
            facing: 0.0,
            motion,
        }
    }

    fn run(jump_at: Option<u32>) -> Motion {
        Motion::Run {
            speed: RUN,
            walk: false,
            jump_at,
        }
    }

    fn claims(track: &Track, until: u32) -> Vec<(u32, Movement)> {
        let ground = Ground::none();
        let mut mover = Mover::new(10.0, 0.0);
        let mut out = Vec::new();
        for t in (0..=until).step_by(50) {
            let mut frame = Vec::new();
            mover.frame(t, track, &ground, &mut frame);
            out.extend(frame.into_iter().map(|m| (t, m)));
        }
        out
    }

    #[test]
    fn a_run_is_its_start_heartbeats_each_half_second_and_its_stop() {
        let track = Track::of(
            vec![
                leg(0, 1000, Motion::Stand),
                leg(1000, 2600, run(None)),
                leg(2600, 4000, Motion::Stand),
            ],
            None,
        );
        let times: Vec<u32> = claims(&track, 4000).iter().map(|c| c.0).collect();
        assert_eq!(times, [1000, 1500, 2000, 2500, 2600]);
    }

    #[test]
    fn looking_about_reports_every_frame_and_a_keyboard_turn_only_its_ends() {
        let look = Motion::MouseLook {
            amplitude: 0.5,
            hz: 0.5,
        };
        let track = Track::of(vec![leg(0, 1000, look)], None);
        assert_eq!(claims(&track, 1000).len(), 20);
        let turned = Leg {
            facing: 3.0,
            ..leg(1000, 2000, Motion::Stand)
        };
        let turn = leg(
            0,
            1000,
            Motion::KeyTurn {
                left_rad_per_s: 3.0,
            },
        );
        let track = Track::of(vec![turn, turned], None);
        let sent = claims(&track, 2000);
        let times: Vec<u32> = sent.iter().map(|c| c.0).collect();
        assert_eq!(times, [0, 500, 1000]);
        assert_eq!(sent[0].1.flags, flags::TURN_LEFT);
        assert!((sent[2].1.facing - 3.0).abs() < 1e-5);
    }

    #[test]
    fn a_jump_launches_falls_and_lands_with_its_fall_time() {
        let track = Track::of(vec![leg(0, 3000, run(Some(1000)))], None);
        let sent = claims(&track, 3000);
        let times: Vec<u32> = sent.iter().map(|c| c.0).collect();
        assert_eq!(times, [0, 500, 1000, 1500, 1850, 2350, 2850]);
        let (launch, land) = (sent[2].1, sent[4].1);
        assert_ne!(launch.flags & flags::FALLING, 0);
        assert!((launch.jump.xy_speed - RUN).abs() < 1e-6);
        assert_eq!(land.flags & flags::FALLING, 0);
        assert_eq!(land.fall_time, 850);
        assert!(
            (sent[3].1.pos[2] - 10.0).abs() > 1.0,
            "mid-air, above the ground"
        );
    }

    #[test]
    fn a_liar_runs_ahead_in_its_window_and_teleports_once() {
        let lie = Lie {
            fast_from: 1000,
            fast_to: 2000,
            teleport_at: 3000,
        };
        let track = Track::of(vec![leg(0, 5000, run(None))], Some(lie));
        let sent = claims(&track, 5000);
        let off = |t: u32, m: &Movement| (m.pos[0] - track.xy(t)[0]).abs();
        let lied: Vec<u32> = sent
            .iter()
            .filter(|(t, m)| off(*t, m) > 0.1)
            .map(|c| c.0)
            .collect();
        assert_eq!(lied, [1500, 3000]);
    }
}
