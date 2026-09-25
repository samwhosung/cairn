use std::f32::consts::TAU;

use libm::{cosf, sinf};
use protocol::{Cadence, Jump, Movement, flags};

use crate::ground::Ground;
use crate::lie::{Lie, Route, Told, same_bits};
use crate::track::Track;

const JUMP_SPEED: f32 = 7.955_547;
const GRAVITY: f32 = 19.291_105;

#[derive(Clone, Copy)]
struct Air {
    launched_at: u32,
    launch_z: f32,
    xy_speed: f32,
    facing: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub truth: Movement,
    pub claims: usize,
    pub lied: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claims {
    ByCadence,
    EveryFrame,
}

pub struct Mover {
    cadence: Cadence,
    last_footing_z: f32,
    air: Option<Air>,
    jumped_leg: Option<usize>,
    lies: Vec<(Lie, Told)>,
    claims: Claims,
    pub ack: u32,
}

struct OnFooting<'a> {
    track: &'a Track,
    ground: &'a Ground,
    fallback_z: f32,
}

impl OnFooting<'_> {
    fn over(&self, [x, y]: [f32; 2]) -> [f32; 3] {
        let z = self.track.footing_z(self.ground, x, y);
        [x, y, z.unwrap_or(self.fallback_z)]
    }
}

impl Route for OnFooting<'_> {
    fn at(&self, t: u32) -> [f32; 3] {
        self.over(self.track.xy(t))
    }

    fn through(&self, t: u32) -> [f32; 3] {
        self.over(self.track.through_xy(t))
    }
}

impl Mover {
    pub fn new(spawn: [f32; 3], facing: f32, lies: Vec<Lie>, claims: Claims) -> Self {
        Self {
            cadence: Cadence::new(&Movement {
                pos: spawn,
                facing: facing.rem_euclid(TAU),
                ..Movement::default()
            }),
            last_footing_z: spawn[2],
            air: None,
            jumped_leg: None,
            lies: lies.into_iter().map(|l| (l, Told::default())).collect(),
            claims,
            ack: 0,
        }
    }

    pub fn correct(&mut self, seq: u32) {
        self.ack = seq;
        self.cadence.report_now();
    }

    /// Moves the body to `t` of its clock and adds the frame's claims to `out`.
    pub fn frame(
        &mut self,
        t: u32,
        track: &Track,
        ground: &Ground,
        out: &mut Vec<Movement>,
    ) -> Frame {
        let truth = self.truth(t, track, ground);
        let route = OnFooting {
            track,
            ground,
            fallback_z: self.last_footing_z,
        };
        let mut claim = truth;
        for (lie, told) in &mut self.lies {
            if lie.holds_at(t) {
                if !told.begun && lie.jumps() {
                    self.cadence.report_now();
                }
                claim = lie.tell(told, t, &claim, &route);
            }
        }
        let mut claims = self.cadence.claims(&claim);
        if self.claims == Claims::EveryFrame {
            claims = claims.max(1);
        }
        if claims > 0 {
            for (lie, told) in &mut self.lies {
                told.shift_sent |= lie.holds_at(t) && lie.shift_once;
            }
        }
        out.extend(std::iter::repeat_n(claim, claims));
        Frame {
            truth,
            claims,
            lied: claims > 0 && !same_bits(&claim, &truth),
        }
    }

    fn truth(&mut self, t: u32, track: &Track, ground: &Ground) -> Movement {
        let pose = track.pose(t);
        let [x, y] = pose.spot.xy;
        let footing_z = track.footing_z(ground, x, y).unwrap_or(self.last_footing_z);
        self.last_footing_z = footing_z;
        let mut live = pose.flags;
        if let Some(jump) = pose.jump
            && t >= jump.at_ms
            && self.air.is_none()
            && self.jumped_leg != Some(pose.leg)
        {
            self.jumped_leg = Some(pose.leg);
            self.air = Some(Air {
                launched_at: jump.at_ms,
                launch_z: footing_z,
                xy_speed: jump.speed,
                facing: pose.spot.facing,
            });
        }
        let mut movement = Movement {
            time: t,
            pos: [x, y, footing_z],
            facing: pose.spot.facing.rem_euclid(TAU),
            ..Movement::default()
        };
        if let Some(air) = self.air {
            let secs = (t - air.launched_at) as f32 / 1000.0;
            let z = air.launch_z + JUMP_SPEED * secs - 0.5 * GRAVITY * secs * secs;
            movement.fall_time = t - air.launched_at;
            if secs > 0.2 && z <= footing_z {
                self.air = None;
            } else {
                live |= flags::FALLING;
                movement.pos[2] = z;
                movement.jump = Jump {
                    z_speed: -JUMP_SPEED,
                    cos: cosf(air.facing),
                    sin: sinf(air.facing),
                    xy_speed: air.xy_speed,
                };
            }
        }
        movement.flags = live;
        movement
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{Gait, Leg, Motion, RUN};

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
            gait: Gait::Run,
            jump_at,
        }
    }

    fn claims(track: &Track, lies: Vec<Lie>, until: u32) -> Vec<(u32, Movement, Movement)> {
        let ground = Ground::flat(10.0);
        let mut mover = Mover::new([0.0, 0.0, 10.0], 0.0, lies, Claims::ByCadence);
        let mut out = Vec::new();
        for t in (0..=until).step_by(50) {
            let mut frame = Vec::new();
            let f = mover.frame(t, track, &ground, &mut frame);
            out.extend(frame.into_iter().map(|m| (t, m, f.truth)));
        }
        out
    }

    fn times(sent: &[(u32, Movement, Movement)]) -> Vec<u32> {
        sent.iter().map(|c| c.0).collect()
    }

    #[test]
    fn a_run_is_its_start_heartbeats_each_half_second_and_its_stop() {
        let track = Track::of(vec![
            leg(0, 1000, Motion::Stand),
            leg(1000, 2600, run(None)),
            leg(2600, 4000, Motion::Stand),
        ]);
        let sent = claims(&track, Vec::new(), 4000);
        assert_eq!(times(&sent), [1000, 1500, 2000, 2500, 2600]);
    }

    #[test]
    fn looking_about_reports_every_frame_and_a_keyboard_turn_only_its_ends() {
        let look = Motion::MouseLook {
            amplitude: 0.5,
            hz: 0.5,
        };
        let track = Track::of(vec![leg(0, 1000, look)]);
        assert_eq!(claims(&track, Vec::new(), 1000).len(), 20);
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
        let sent = claims(&Track::of(vec![turn, turned]), Vec::new(), 2000);
        assert_eq!(times(&sent), [0, 500, 1000]);
        assert_eq!(sent[0].1.flags, flags::TURN_LEFT);
        assert!((sent[2].1.facing - 3.0).abs() < 1e-5);
    }

    #[test]
    fn a_jump_launches_falls_and_lands_with_its_fall_time() {
        let track = Track::of(vec![leg(0, 3000, run(Some(1000)))]);
        let sent = claims(&track, Vec::new(), 3000);
        assert_eq!(times(&sent), [0, 500, 1000, 1500, 1850, 2350, 2850]);
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
    fn a_crowd_liar_runs_ahead_in_its_window_and_teleports_once() {
        let fast = Lie {
            from_ms: 1000,
            to_ms: 2000,
            factor: Some(3.0),
            ..Lie::default()
        };
        let teleport = Lie {
            from_ms: 3000,
            shift: [200.0, 0.0, 0.0],
            shift_once: true,
            ..Lie::default()
        };
        let track = Track::of(vec![leg(0, 5000, run(None))]);
        let sent = claims(&track, vec![fast, teleport], 5000);
        let lied: Vec<u32> = sent
            .iter()
            .filter(|(_, claim, truth)| (claim.pos[0] - truth.pos[0]).abs() > 0.1)
            .map(|c| c.0)
            .collect();
        assert_eq!(lied, [1500, 3000]);
        assert!(sent.iter().all(|c| c.1.time == c.0), "an honest clock");
    }

    #[test]
    fn a_jump_goes_out_the_frame_it_begins_and_every_frame_when_told_to() {
        let track = Track::of(vec![leg(0, 5000, run(None))]);
        let under = Lie {
            from_ms: 1250,
            shift: [0.0, 0.0, -10.0],
            ..Lie::default()
        };
        let sent = claims(&track, vec![under], 2000);
        assert_eq!(times(&sent), [0, 500, 1000, 1250, 1750]);
        assert!(sent[3].1.pos[2].abs() < 1e-6, "ten yards under the ground");
        let ground = Ground::flat(10.0);
        let mut mover = Mover::new([0.0, 0.0, 10.0], 0.0, Vec::new(), Claims::EveryFrame);
        let mut out = Vec::new();
        for t in (0..1000).step_by(50) {
            mover.frame(t, &track, &ground, &mut out);
        }
        assert_eq!(out.len(), 20);
    }
}
