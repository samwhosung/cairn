//! When another player's relayed move applies, on this window's clock.

use protocol::flags;

const SKEW_MIN_MS: f64 = -500.0;
const SKEW_MAX_MS: f64 = 1000.0;
const LATENESS_WINDOW: usize = 32;

#[derive(Clone, Debug, Default)]
pub struct ReplayTiming {
    seeded: bool,
    last_fire_ms: f64,
    last_server_ms: u32,
    needed_ms: [f64; LATENESS_WINDOW],
    needed_at: usize,
    buffer_ms: f64,
}

impl ReplayTiming {
    /// The time on this window's clock to apply a move stamped `server_ms` that arrived at
    /// `arrived_ms`, given the player's flags and whether nothing of theirs waits, both from
    /// before the move applies.
    pub fn schedule(
        &mut self,
        server_ms: u32,
        arrived_ms: f64,
        flags: u32,
        queue_empty: bool,
    ) -> f64 {
        if !self.seeded {
            self.seeded = true;
            self.last_server_ms = server_ms;
            self.last_fire_ms = arrived_ms;
        }
        let step = server_ms.wrapping_sub(self.last_server_ms) as i32;
        let server_delta = if step > 0 {
            self.last_server_ms = server_ms;
            f64::from(step)
        } else {
            0.0
        };
        let arrival_delta = arrived_ms - self.last_fire_ms;
        let mut skew = server_delta - arrival_delta;
        let widest = self.widest_need(arrival_delta - server_delta);
        if flags & flags::UNDER_WAY == 0 && queue_empty {
            skew = (skew + widest - self.buffer_ms).clamp(SKEW_MIN_MS, SKEW_MAX_MS);
            if arrived_ms + skew < self.last_fire_ms {
                skew = self.last_fire_ms - arrived_ms;
            }
            self.buffer_ms = widest;
        }
        let fire_ms = arrived_ms + skew.clamp(SKEW_MIN_MS, SKEW_MAX_MS);
        self.last_fire_ms = fire_ms;
        fire_ms
    }

    fn widest_need(&mut self, lateness_ms: f64) -> f64 {
        self.needed_ms[self.needed_at] = self.buffer_ms + lateness_ms;
        self.needed_at = (self.needed_at + 1) % LATENESS_WINDOW;
        self.needed_ms
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: u32 = 0x1;

    #[test]
    fn the_first_move_fires_on_arrival_and_a_mover_replays_at_its_stamps_spacing() {
        let mut chain = ReplayTiming::default();
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
        let mut chain = ReplayTiming::default();
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
        let mut chain = ReplayTiming::default();
        chain.schedule(0, 0.0, RUN, false);
        let fire = chain.schedule(60_000, 100.0, RUN, false);
        assert!((fire - (100.0 + SKEW_MAX_MS)).abs() < 1e-9, "{fire}");
        let fire = chain.schedule(60_000, 5000.0, RUN, false);
        assert!((fire - (5000.0 + SKEW_MIN_MS)).abs() < 1e-9, "{fire}");
    }
}
