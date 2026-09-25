//! One clock for a test's server and its windows: a window's frame is a step of test time, and the
//! server ticks as test time reaches each of its ticks. Nothing sleeps, so no verdict depends on
//! how fast the machine runs.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use bevy::prelude::App;
use bevy::time::TimeUpdateStrategy;
use game::Delivery;
use server::{Config, InProcess, InputOrder, Stepper, Summary, TickStats};

/// Ticks the server runs after the windows' last frames, so that everything they sent is judged.
const TICKS_TO_JUDGE: usize = 2;

pub struct Clock {
    server: Option<Stepper>,
    epoch: Instant,
    step: Duration,
    tick: Duration,
    now: Duration,
    next_tick: Duration,
    ticks: Vec<TickStats>,
    threads: usize,
}

/// A test's clock, shared by its windows.
pub type Served = Rc<RefCell<Clock>>;

/// A frame's step at `hz` frames a second.
pub fn step_at(hz: f32) -> Duration {
    Duration::from_secs_f64(1.0 / f64::from(hz))
}

/// Serves `cfg` on a clock its windows move on `step` at a time.
pub fn serve(cfg: &Config, step: Duration) -> Served {
    let server = Stepper::new(cfg, InputOrder::Canonical, Delivery::Canonical).expect("a server");
    let tick = Duration::from_millis(u64::from(cfg.tick_ms));
    Rc::new(RefCell::new(Clock {
        server: Some(server),
        epoch: Instant::now(),
        step,
        tick,
        now: Duration::ZERO,
        next_tick: tick,
        ticks: Vec::new(),
        threads: cfg.tick_threads,
    }))
}

impl Clock {
    /// A connection to the server, as its host, who may teleport, or as a guest.
    pub fn connect(&mut self, host: bool) -> InProcess {
        self.server
            .as_mut()
            .expect("the server runs")
            .connect_in_process(host)
    }

    pub fn step(&self) -> Duration {
        self.step
    }

    /// Test time as the windows' clocks read it.
    pub fn instant(&self) -> Instant {
        self.epoch + self.now
    }

    pub fn ms(&self) -> u32 {
        self.now.as_millis() as u32
    }

    /// Moves test time on a step. The server first reads what was sent until now, then ticks at
    /// each tick the new time reaches, and its frames arrive at the tick's time.
    fn advance(&mut self) {
        let received_ms = self.ms();
        self.now += self.step;
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

/// A window's frames on a clock.
pub struct Stepping {
    clock: Served,
    last: Option<Duration>,
}

impl Stepping {
    pub fn on(clock: &Served) -> Self {
        Self {
            clock: clock.clone(),
            last: None,
        }
    }

    /// Runs `app`'s next frame. A window whose last frame was at the clock's time moves the clock
    /// on a step first; any other catches up to it, as the other windows of a round do.
    pub fn frame(&mut self, app: &mut App) {
        let at = {
            let mut clock = self.clock.borrow_mut();
            if self.last == Some(clock.now) {
                clock.advance();
            }
            self.last = Some(clock.now);
            clock.instant()
        };
        app.insert_resource(TimeUpdateStrategy::ManualInstant(at));
        app.update();
    }

    /// Runs a frame of `app` at the clock's time, which moves nothing on: the frames a world takes
    /// to stream in, or a shot to come back.
    pub fn hold(&mut self, app: &mut App) {
        let at = {
            let clock = self.clock.borrow();
            self.last = Some(clock.now);
            clock.instant()
        };
        app.insert_resource(TimeUpdateStrategy::ManualInstant(at));
        app.update();
    }
}
