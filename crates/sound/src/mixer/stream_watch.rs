//! A starved stream zero-fills whole blocks while the mix meets every deadline, so no other meter
//! sees it; its position freezes while it stays audible. The watch compares wall time against the
//! position's advance over each window.
//!
//! Every stream opens frozen: kira reports it playing at position 0 before the decoder has two
//! frames ready. That is latency before the first sample, not silence cut into audio, so nothing
//! is counted until the position first moves.

use std::time::Instant;

use bevy::log::{debug, warn};
use kira::sound::FromFileError;

use super::StreamingSoundHandle;

/// More than two lost ~43 ms blocks per window reports; real starvation runs hundreds of ms.
const STARVED_MIN_SECS: f64 = 0.1;
const WINDOW_SECS: f64 = 1.0;
/// A stream still pinned at its start this long after going audible never played.
const START_MAX_SECS: f64 = 3.0;

pub struct StreamWatch {
    label: &'static str,
    phase: Phase,
    expected: f64,
    advanced: f64,
}

#[derive(Clone, Copy)]
enum Phase {
    Idle,
    Starting {
        since: Instant,
        pos: f64,
        warned: bool,
    },
    /// `window_start` is stamped with the counting's own first instant, so the counted time and
    /// the reported span measure one interval.
    Running {
        last_pos: f64,
        window_start: Instant,
    },
}

pub(super) enum Verdict {
    Starved {
        lost: f64,
        counted: f64,
        advanced: f64,
        span: f64,
    },
    NeverStarted {
        waited: f64,
    },
    Began {
        after: f64,
    },
}

impl StreamWatch {
    pub fn new(label: &'static str) -> Self {
        Self {
            label,
            phase: Phase::Idle,
            expected: 0.0,
            advanced: 0.0,
        }
    }

    /// Call every frame the handle exists, with the wall-clock delta.
    pub fn feed(&mut self, handle: &StreamingSoundHandle<FromFileError>, dt: f64) {
        use kira::sound::PlaybackState as S;
        let audible = matches!(handle.state(), S::Playing | S::Stopping);
        match self.observe(audible, handle.position(), dt, Instant::now()) {
            Some(Verdict::Starved {
                lost,
                counted,
                advanced,
                span,
            }) => warn!(
                "audio: {} stream starved: ~{:.0} ms of silence over a {span:.2} s window \
                 (counted {counted:.2} s, advanced {advanced:.2} s)",
                self.label,
                lost * 1000.0,
            ),
            Some(Verdict::NeverStarted { waited }) => warn!(
                "audio: {} has been audible {waited:.1} s with its position still at the start",
                self.label,
            ),
            Some(Verdict::Began { after }) => debug!(
                "audio: {} began advancing {:.0} ms after the watch first saw it",
                self.label,
                after * 1000.0,
            ),
            None => {}
        }
    }

    /// Forget the baseline: the next stream starts behind the old one's position.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    fn begin(&mut self, pos: f64, now: Instant) {
        self.phase = Phase::Running {
            last_pos: pos,
            window_start: now,
        };
        self.expected = 0.0;
        self.advanced = 0.0;
    }

    pub(super) fn observe(
        &mut self,
        audible: bool,
        pos: f64,
        dt: f64,
        now: Instant,
    ) -> Option<Verdict> {
        if !audible {
            self.reset();
            return None;
        }
        match self.phase {
            Phase::Idle => {
                self.phase = Phase::Starting {
                    since: now,
                    pos,
                    warned: false,
                };
                None
            }
            Phase::Starting {
                since,
                pos: start,
                warned,
            } => {
                let waited = now.duration_since(since).as_secs_f64();
                if pos > start {
                    self.begin(pos, now);
                    return Some(Verdict::Began { after: waited });
                }
                if !warned && waited > START_MAX_SECS {
                    self.phase = Phase::Starting {
                        since,
                        pos: start,
                        warned: true,
                    };
                    return Some(Verdict::NeverStarted { waited });
                }
                None
            }
            Phase::Running {
                last_pos,
                window_start,
            } => {
                // A stream swapped onto the slot starts behind the old one: a new stream.
                if pos < last_pos {
                    self.phase = Phase::Starting {
                        since: now,
                        pos,
                        warned: false,
                    };
                    self.expected = 0.0;
                    self.advanced = 0.0;
                    return None;
                }
                self.expected += dt;
                self.advanced += pos - last_pos;
                self.phase = Phase::Running {
                    last_pos: pos,
                    window_start,
                };
                if self.expected < WINDOW_SECS {
                    return None;
                }
                let (counted, advanced) = (self.expected, self.advanced);
                let span = now.duration_since(window_start).as_secs_f64();
                self.begin(pos, now);
                let lost = counted - advanced;
                (lost > STARVED_MIN_SECS).then_some(Verdict::Starved {
                    lost,
                    counted,
                    advanced,
                    span,
                })
            }
        }
    }
}
