//! A walker's own server, run in-process as a bare window runs one: joining it, holding the
//! walker's frames to its clock, and what it made of the walk.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::App;
use protocol::Why;
use server::Summary;
use world::unit::CharacterLook;

use crate::net::{self, Net};
use crate::view::Pose;

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

pub struct Judged {
    pub refused: Vec<(Why, u64)>,
    pub corrections: u32,
    pub claims: u32,
    pub teleports: u32,
    pub summary: Summary,
}

impl std::fmt::Debug for Judged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let [p50, p99, _] = self.summary.cpu;
        write!(
            f,
            "refused {:?}, {} corrections, {} claims, {} teleports; a tick took {p50:.3} / \
             {p99:.3} ms of CPU (p50 / p99)",
            self.refused, self.corrections, self.claims, self.teleports
        )
    }
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
    let two_ticks = net.welcome().map_or(0, |w| 2 * u64::from(w.tick_ms));
    std::thread::sleep(Duration::from_millis(two_ticks));
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
        summary,
    };
    eprintln!("its own server: {judged:?}");
    Some(judged)
}

/// Fails unless `app`'s own server refused nothing of its walk and never put it back.
pub fn assert_honest(app: &mut App, who: &str) {
    if std::thread::panicking() {
        return;
    }
    let judged = judge(app);
    assert!(
        judged.as_ref().is_some_and(Judged::honest),
        "the {who}'s own server put it back: {judged:?}"
    );
}
