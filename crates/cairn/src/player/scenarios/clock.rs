//! One clock for a test's server and its windows: a window's frame is a step of test time, and the
//! server ticks as test time reaches each of its ticks. Nothing sleeps, so no verdict depends on
//! how fast the machine runs.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use bevy::prelude::App;
use bevy::time::TimeUpdateStrategy;
use game::Delivery;
use server::{Config, InProcess, InputOrder, Standing, Stepper, Summary, TickStats};

const TICKS_TO_JUDGE: usize = 2;

pub struct Clock {
    server: Option<Stepper>,
    epoch: Instant,
    frame_step: Duration,
    tick: Duration,
    now: Duration,
    next_tick: Duration,
    ticks: Vec<TickStats>,
    threads: usize,
}

pub type SharedClock = Rc<RefCell<Clock>>;

pub fn step_at(hz: f32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(hz))
}

pub fn serve(cfg: &Config, frame_step: Duration) -> SharedClock {
    let server = Stepper::new(cfg, InputOrder::Canonical, Delivery::Canonical).expect("a server");
    let tick = Duration::from_millis(u64::from(cfg.tick_ms));
    Rc::new(RefCell::new(Clock {
        server: Some(server),
        epoch: Instant::now(),
        frame_step,
        tick,
        now: Duration::ZERO,
        next_tick: tick,
        ticks: Vec::new(),
        threads: cfg.tick_threads,
    }))
}

impl Clock {
    pub fn connect(&mut self, standing: Standing) -> InProcess {
        self.server
            .as_mut()
            .expect("the server runs")
            .connect_in_process(standing)
    }

    pub fn frame_step(&self) -> Duration {
        self.frame_step
    }

    fn instant(&self) -> Instant {
        self.epoch + self.now
    }

    pub fn ms(&self) -> u32 {
        self.now.as_millis() as u32
    }

    fn advance(&mut self) {
        let received_ms = self.ms();
        self.now += self.frame_step;
        let Some(server) = &mut self.server else {
            return;
        };
        server.receive(received_ms);
        while self.next_tick <= self.now {
            self.ticks.push(server.tick(&[]));
            server.hand_over(self.epoch + self.next_tick);
            self.next_tick += self.tick;
        }
    }

    /// Stops the server once what the windows sent has been judged, and says what it made of the
    /// run.
    pub fn stop(&mut self) -> Summary {
        let judged = self.ticks.len() + TICKS_TO_JUDGE;
        while self.ticks.len() < judged {
            self.advance();
        }
        let server = self.server.take().expect("the server runs");
        let bytes_in = server.bytes_in();
        server.finish().expect("the server stops");
        let secs = self.tick.as_secs_f64() * self.ticks.len() as f64;
        Summary::of(&self.ticks, self.threads, secs, bytes_in, 0)
    }
}

/// A window's frames, on a test's clock or on the wall's.
pub enum Frames {
    Stepped {
        clock: SharedClock,
        last: Option<Duration>,
    },
    OnTheWall,
}

impl Frames {
    pub fn on(clock: &SharedClock) -> Self {
        Self::Stepped {
            clock: clock.clone(),
            last: None,
        }
    }

    pub fn clock(&self) -> Option<&SharedClock> {
        match self {
            Self::Stepped { clock, .. } => Some(clock),
            Self::OnTheWall => None,
        }
    }

    /// Runs `app`'s next frame. On a test's clock, a window whose last frame was at the clock's
    /// time moves the clock on a step first; any other catches up to it, as the other windows of a
    /// round do.
    pub fn frame(&mut self, app: &mut App) {
        let Self::Stepped { clock, last } = self else {
            app.update();
            return;
        };
        let at = {
            let mut clock = clock.borrow_mut();
            if *last == Some(clock.now) {
                clock.advance();
            }
            *last = Some(clock.now);
            clock.instant()
        };
        app.insert_resource(TimeUpdateStrategy::ManualInstant(at));
        app.update();
    }

    /// Runs a frame of `app` that moves a test's clock nothing on: the frames a world takes to
    /// stream in, or a shot to come back.
    pub fn hold(&mut self, app: &mut App) {
        let Self::Stepped { clock, last } = self else {
            app.update();
            return;
        };
        let at = {
            let clock = clock.borrow();
            *last = Some(clock.now);
            clock.instant()
        };
        app.insert_resource(TimeUpdateStrategy::ManualInstant(at));
        app.update();
    }

    /// Waits out `step` after a frame on the wall clock; on a test's, nothing.
    pub fn wait(&self, step: Duration) {
        if let Self::OnTheWall = self {
            std::thread::sleep(step);
        }
    }
}
