use bevy::prelude::{Resource, Vec3};

/// A C runtime `rand()` stream that animation arms roll their variations and replay counts from.
#[derive(Resource, Clone, Copy, Debug)]
pub struct AnimRng {
    state: u32,
    seed: u32,
}

impl Default for AnimRng {
    /// The C runtime's seed before `srand`.
    fn default() -> Self {
        Self::seeded(1)
    }
}

impl AnimRng {
    fn seeded(seed: u32) -> Self {
        Self { state: seed, seed }
    }

    /// One draw, `0..=0x7fff`.
    pub fn draw(&mut self) -> u16 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.state >> 16) & 0x7fff) as u16
    }

    /// The passes an arm plays before it is re-rolled: `max(1, min + ((rand() · (max − min)) >>
    /// 15))`. The draw happens even when the range cannot change the answer.
    pub fn replay_count(&mut self, replay: (u32, u32)) -> u32 {
        let (lo, hi) = replay;
        let r = lo + ((u64::from(self.draw()) * u64::from(hi.saturating_sub(lo))) >> 15) as u32;
        r.max(1)
    }

    /// Seeds from the wall clock, as the client does at startup, unless a reproducible run is
    /// wanted.
    pub fn seed_for_session(&mut self, reproducible: bool) {
        if reproducible {
            return;
        }
        *self = Self::seeded(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_millis() as u32),
        );
    }

    /// A stream for an arm standing at `at`, seeded from the seed this one started from.
    #[must_use]
    pub fn at(&self, at: Vec3) -> Self {
        let seed = at
            .to_array()
            .into_iter()
            .fold(self.seed, |h, v| murmur3_fmix32(h ^ v.to_bits()));
        Self::seeded(seed)
    }
}

fn murmur3_fmix32(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^ (h >> 16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_draw_is_the_c_runtime_lcg() {
        let mut rng = AnimRng::default();
        let first: Vec<u16> = (0..6).map(|_| rng.draw()).collect();
        assert_eq!(first, vec![41, 18_467, 6_334, 26_500, 19_169, 15_724]);
    }

    #[test]
    fn a_zero_replay_range_is_one_pass_and_still_draws() {
        let mut rng = AnimRng::default();
        assert_eq!(rng.replay_count((0, 0)), 1);
        let mut bare = AnimRng::default();
        bare.draw();
        assert_eq!(rng.draw(), bare.draw());
    }

    #[test]
    fn the_replay_count_scales_with_the_range() {
        let mut rng = AnimRng::default();
        let counts: Vec<u32> = (0..200).map(|_| rng.replay_count((3, 10))).collect();
        assert!(counts.iter().all(|&r| (3..10).contains(&r)));
        assert!(counts.contains(&3) && counts.contains(&9));
    }
}
