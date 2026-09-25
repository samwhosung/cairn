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
    /// Animations played once that the bots were told of, and poses held or let go.
    pub plays_told: u64,
    pub poses_told: u64,
    /// Animations played once on a body out of a bot's view, which it must not be told of: one a
    /// bot for each bot that did not see it.
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
            plays_out_of_view: 0,
        }
    }

    pub fn check(&mut self, tick: u32, hash: u64, stepper: &Stepper, clients: &[Client]) {
        self.hash_chain = (self.hash_chain ^ hash).wrapping_mul(CHAIN_PRIME);
        if let Some(what) = stepper.saves_differ() {
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
            let on_it = to_it.iter().filter(|(slot, _)| slot.is_none()).count();
            let own = stepper.game().map_or(0, |g| {
                let shows = &g.shows().played;
                shows.iter().filter(|&&(n, _)| n == c.conn).count()
            });
            self.plays_out_of_view += (played - own - (to_it.len() - on_it)) as u64;
            let own_pose = stepper.pose_of(c.conn);
            if let Some(what) = c.shown.differs(tick, &view, own_pose, &to_it) {
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
            let (played, held) = c.shown.told();
            self.plays_told += played;
            self.poses_told += held;
        }
    }
}
