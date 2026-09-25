use game::Loaded;
use server::Stepper;

use super::client::Client;
use super::shown::ServerOwnShows;

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
    pub plays_told: u64,
    pub poses_told: u64,
    pub idles_told: u64,
    pub attacks_told: u64,
    pub plays_out_of_view: u64,
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
            plays_told: 0,
            poses_told: 0,
            idles_told: 0,
            attacks_told: 0,
            plays_out_of_view: 0,
        }
    }

    pub fn check(&mut self, tick: u32, hash: u64, stepper: &Stepper, clients: &[Client]) {
        self.hash_chain = (self.hash_chain ^ hash).wrapping_mul(CHAIN_PRIME);
        if let Some(what) = stepper.file_differs() {
            self.saved_mismatches += 1;
            self.first_saved_mismatch
                .get_or_insert_with(|| format!("tick {tick}: {what}"));
        }
        let played = stepper.game().map_or(0, |g| g.shows().played.len());
        for c in clients {
            let Some(view) = stepper.in_view(c.conn) else {
                continue;
            };
            let to_it = stepper.played_to(c.conn);
            self.plays_out_of_view += (played - to_it.len()) as u64;
            let own = ServerOwnShows {
                pose: stepper.pose_of(c.conn),
                idle: stepper.idle_of(c.conn),
            };
            let attacked = stepper.attacks_to(c.conn);
            if let Some(what) = c
                .shown
                .first_difference(tick, &view, &own, &to_it, &attacked)
            {
                self.shown_mismatches += 1;
                self.first_shown_mismatch
                    .get_or_insert_with(|| format!("tick {tick}: bot {}: {what}", c.conn));
            }
        }
    }

    pub fn finish(&mut self, stepper: &Stepper, clients: &[Client]) {
        if let Some(game) = stepper.game() {
            self.counts = game.counts().iter().map(|(&k, &v)| (k, v)).collect();
        }
        for c in clients {
            self.plays_told += c.shown.plays_told();
            self.poses_told += c.shown.poses_told();
            self.idles_told += c.shown.idles_told();
            self.attacks_told += c.shown.attacks_told();
        }
    }
}
