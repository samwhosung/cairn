use game::Loaded;
use server::Stepper;

use super::client::Client;

const CHAIN_START: u64 = 0xcbf2_9ce4_8422_2325;
const CHAIN_PRIME: u64 = 0x0100_0000_01b3;

#[derive(Clone, Debug)]
pub struct Played {
    pub name: &'static str,
    pub counts: Vec<(&'static str, i64)>,
    pub hash_chain: u64,
    pub saved_mismatches: u64,
    pub first_saved_mismatch: Option<String>,
    pub shown_mismatches: u64,
    pub first_shown_mismatch: Option<String>,
}

impl Played {
    pub fn zeroed(game: &Loaded) -> Self {
        Self {
            name: game.name(),
            counts: game.counts().iter().map(|&what| (what, 0)).collect(),
            hash_chain: CHAIN_START,
            saved_mismatches: 0,
            first_saved_mismatch: None,
            shown_mismatches: 0,
            first_shown_mismatch: None,
        }
    }

    pub fn check(&mut self, tick: u32, hash: u64, stepper: &Stepper, clients: &[Client]) {
        self.hash_chain = (self.hash_chain ^ hash).wrapping_mul(CHAIN_PRIME);
        if let Some(what) = stepper.saves_differ() {
            self.saved_mismatches += 1;
            self.first_saved_mismatch
                .get_or_insert_with(|| format!("tick {tick}: {what}"));
        }
        for c in clients {
            let Some(view) = stepper.in_view(c.conn) else {
                continue;
            };
            if let Some(what) = c.shown.differs(&view) {
                self.shown_mismatches += 1;
                self.first_shown_mismatch
                    .get_or_insert_with(|| format!("tick {tick}: bot {}: {what}", c.conn));
            }
        }
    }

    pub fn finish(&mut self, stepper: &Stepper) {
        if let Some(game) = stepper.game() {
            self.counts = game.counts().iter().map(|(&k, &v)| (k, v)).collect();
        }
    }
}
