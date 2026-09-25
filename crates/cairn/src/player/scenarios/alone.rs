//! A walker's own server, run in-process as a bare window runs one: joining it, holding the
//! walker's frames to its clock, and what it made of the walk.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::App;
use protocol::Why;
use world::unit::CharacterLook;

use crate::net::{self, Net};
use crate::view::Pose;

/// Long enough for claims already sent to reach a tick of a server ticking at 20 Hz.
const TWO_TICKS: Duration = Duration::from_millis(100);

/// Serves `map` to one player whose body starts at `pose`, as a bare window does, and joins it as
/// `look`; `record` names where the server writes its inputs.
pub fn own_server(map: u32, pose: Pose, look: &CharacterLook, record: Option<PathBuf>) -> Net {
    let mut cfg = net::own_server(None, map, pose.target.to_array(), pose.heading);
    cfg.record = record;
    Net::host(cfg, net::hello("Walker".into(), look)).expect("the walker's own server")
}

/// Holds frames that each step the game clock by `step` to no faster than the wall clock, as a
/// window's are: a claim's clock may fall behind its server's, never run ahead of it.
#[derive(Default)]
pub struct Pace {
    last: Option<Instant>,
}

impl Pace {
    pub fn start(&mut self) {
        self.last = Some(Instant::now());
    }

    pub fn wait(&mut self, step: Duration) {
        if let Some(last) = &mut self.last {
            std::thread::sleep((*last + step).saturating_duration_since(Instant::now()));
            *last = Instant::now();
        }
    }
}

/// What a walker's own server made of its walk.
#[derive(Debug)]
pub struct Judged {
    /// Claims and teleports refused, by why.
    pub refused: Vec<(Why, u64)>,
    /// How often the client was put back.
    pub corrections: u32,
    pub claims: u32,
    pub teleports: u32,
}

impl Judged {
    pub fn honest(&self) -> bool {
        self.refused.is_empty() && self.corrections == 0
    }
}

/// Stops `app`'s own server once the claims sent have reached a tick and says what it made of
/// them; `None` if the app has no server of its own left.
pub fn judge(app: &mut App) -> Option<Judged> {
    let mut net = app.world_mut().get_resource_mut::<Net>()?;
    std::thread::sleep(TWO_TICKS);
    let summary = net.stop_hosted()?.expect("the server stops");
    let judged = Judged {
        refused: Why::ALL
            .into_iter()
            .zip(summary.refused)
            .filter(|&(_, n)| n > 0)
            .collect(),
        corrections: net.corrections(),
        claims: net.claims_sent(),
        teleports: net.teleports_sent(),
    };
    eprintln!("its own server: {judged:?}");
    Some(judged)
}
