use crate::{Id, Tick};

const SPREAD: u64 = 0x9e37_79b9_7f4a_7c15;

fn scramble(mut z: u64) -> u64 {
    z ^= z >> 33;
    z = z.wrapping_mul(0xff51_afd7_ed55_8ccd);
    z ^= z >> 33;
    z = z.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    z ^ (z >> 33)
}

pub fn roll(seed: u64, tick: Tick, id: Id, salt: u32) -> u64 {
    let who = (u64::from(id.kind) << 32) | u64::from(id.n);
    [u64::from(tick), who, u64::from(salt)]
        .into_iter()
        .fold(scramble(seed), |h, word| {
            scramble(h ^ word.wrapping_mul(SPREAD))
        })
}

pub fn within(roll: u64, lo: u32, hi: u32) -> u32 {
    if hi <= lo {
        return lo;
    }
    lo + (roll % (u64::from(hi - lo) + 1)) as u32
}

pub fn chance(roll: u64, p: f64) -> bool {
    ((roll >> 11) as f64) < p * (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_roll_is_fixed_by_what_it_names_and_each_name_moves_it() {
        let at = roll(7, 100, Id::player(3), 1);
        assert_eq!(at, roll(7, 100, Id::player(3), 1));
        for other in [
            roll(8, 100, Id::player(3), 1),
            roll(7, 101, Id::player(3), 1),
            roll(7, 100, Id::player(4), 1),
            roll(7, 100, Id { kind: 1, n: 3 }, 1),
            roll(7, 100, Id::player(3), 2),
        ] {
            assert_ne!(at, other);
        }
    }

    #[test]
    fn rolls_spread_over_their_range() {
        let mut seen = [0u32; 5];
        let mut heads = 0;
        for t in 0..10_000 {
            let r = roll(1, t, Id::player(0), 0);
            seen[(within(r, 10, 14) - 10) as usize] += 1;
            heads += u32::from(chance(r, 0.25));
        }
        assert!(seen.iter().all(|&n| (1800..2200).contains(&n)), "{seen:?}");
        assert!((2300..2700).contains(&heads), "{heads}");
        assert_eq!(within(5, 3, 3), 3);
    }
}
