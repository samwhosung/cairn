//! When another player's relayed move is applied: each move is given a time to fire on this
//! window's clock, paced by the server's own stamps so the moves replay at the spacing they were
//! made, and a buffer that absorbs how they bunched in flight is resized only while the player
//! stands still with nothing waiting.

/// A fire time lands at most this far before its move's arrival, ms…
const SKEW_MIN_MS: f64 = -500.0;
/// …and at most this far after it.
const SKEW_MAX_MS: f64 = 1000.0;
/// How many moves back the worst lateness is remembered.
const LATENESS_WINDOW: usize = 32;
/// Direction, turn and fall bits: while any is set the chain keeps the sender's pacing.
const BUSY: u32 = 0x20ff;

/// One remote player's replay timing.
#[derive(Clone, Debug, Default)]
pub struct RelayChain {
    seeded: bool,
    last_fire_ms: f64,
    last_wire_ms: u32,
    /// How late each recent move ran against the chain, each against the base as it stood then,
    /// so a spike the base has absorbed is not charged twice.
    ring: [f64; LATENESS_WINDOW],
    ring_at: usize,
    /// The buffer the chain holds now.
    base_ms: f64,
}

impl RelayChain {
    /// The time on this window's clock to apply a move stamped `wire_ms` that arrived at
    /// `now_ms`, given the player's flags and whether nothing of theirs waits, both from before
    /// the move applies.
    pub fn schedule(&mut self, wire_ms: u32, now_ms: f64, flags: u32, queue_empty: bool) -> f64 {
        if !self.seeded {
            self.seeded = true;
            self.last_wire_ms = wire_ms;
            self.last_fire_ms = now_ms;
        }
        let step = wire_ms.wrapping_sub(self.last_wire_ms) as i32;
        let wire_delta = if step > 0 {
            self.last_wire_ms = wire_ms;
            f64::from(step)
        } else {
            0.0
        };
        let arrival_delta = now_ms - self.last_fire_ms;
        let mut skew = wire_delta - arrival_delta;
        let window_max = self.record_lateness(arrival_delta - wire_delta);
        if flags & BUSY == 0 && queue_empty {
            skew = (skew + window_max - self.base_ms).clamp(SKEW_MIN_MS, SKEW_MAX_MS);
            if now_ms + skew < self.last_fire_ms {
                skew = self.last_fire_ms - now_ms;
            }
            self.base_ms = window_max;
        }
        let fire_ms = now_ms + skew.clamp(SKEW_MIN_MS, SKEW_MAX_MS);
        self.last_fire_ms = fire_ms;
        fire_ms
    }

    fn record_lateness(&mut self, lateness_ms: f64) -> f64 {
        self.ring[self.ring_at] = self.base_ms + lateness_ms;
        self.ring_at = (self.ring_at + 1) % LATENESS_WINDOW;
        self.ring.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: u32 = 0x1;

    #[test]
    fn the_first_move_fires_on_arrival_and_a_mover_replays_at_its_stamps_spacing() {
        let mut chain = RelayChain::default();
        assert!((chain.schedule(10_000, 500.0, 0, true) - 500.0).abs() < 1e-9);
        let bunched = [(10_050, 580.0), (10_100, 581.0), (10_150, 660.0)];
        let fires: Vec<f64> = bunched
            .iter()
            .map(|&(wire, now)| chain.schedule(wire, now, RUN, false))
            .collect();
        assert_eq!(fires, [550.0, 600.0, 650.0], "paced by the stamps");
    }

    #[test]
    fn a_standing_player_buffers_the_worst_lateness_it_has_seen() {
        let mut chain = RelayChain::default();
        chain.schedule(0, 0.0, 0, true);
        chain.schedule(50, 70.0, RUN, false);
        let fire = chain.schedule(100, 100.0, 0, true);
        assert!(
            (fire - 120.0).abs() < 1e-9,
            "20 ms late once, 20 ms buffered: {fire}"
        );
    }

    #[test]
    fn a_fire_time_never_strays_more_than_the_skew_allows() {
        let mut chain = RelayChain::default();
        chain.schedule(0, 0.0, RUN, false);
        let fire = chain.schedule(60_000, 100.0, RUN, false);
        assert!((fire - (100.0 + SKEW_MAX_MS)).abs() < 1e-9, "{fire}");
        let fire = chain.schedule(60_000, 5000.0, RUN, false);
        assert!((fire - (5000.0 + SKEW_MIN_MS)).abs() < 1e-9, "{fire}");
    }
}
