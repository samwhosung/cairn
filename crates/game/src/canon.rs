use std::hash::{Hash, Hasher};

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0100_0000_01b3;

pub struct StableHasher(u64);

impl Default for StableHasher {
    fn default() -> Self {
        Self(OFFSET)
    }
}

impl Hasher for StableHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(PRIME);
        }
    }

    fn write_usize(&mut self, n: usize) {
        self.write(&(n as u64).to_le_bytes());
    }

    fn write_isize(&mut self, n: isize) {
        self.write(&(n as i64).to_le_bytes());
    }
}

pub fn hash(value: &impl Hash) -> u64 {
    let mut h = StableHasher::default();
    value.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Hash)]
    struct Row {
        health: u32,
        dead: bool,
        target: Option<u32>,
    }

    #[test]
    fn every_field_moves_the_hash_and_equal_rows_hash_alike() {
        let row = |health, dead, target| {
            hash(&Row {
                health,
                dead,
                target,
            })
        };
        let base = row(10, false, None);
        assert_eq!(base, row(10, false, None));
        for other in [
            row(11, false, None),
            row(10, true, None),
            row(10, false, Some(0)),
        ] {
            assert_ne!(base, other);
        }
    }
}
