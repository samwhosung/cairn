use std::collections::BTreeMap;

use std::num::NonZeroU64;

use protocol::{LEN_BYTES, Record, ServerMessage, Show};
use server::InView;

/// Which of what it is shown a bot drops: a control that the checks of it can fail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drops {
    /// The nth game state.
    pub state: Option<NonZeroU64>,
    /// The nth animation played once.
    pub played: Option<NonZeroU64>,
    /// The nth pose held or let go.
    pub held: Option<NonZeroU64>,
}

#[derive(Default)]
struct Held {
    id: u32,
    state: Option<Vec<u8>>,
    pose: Option<u16>,
}

/// A bot's model of what the server shows it: the game's state of each entity in view, the pose
/// each body holds and its own, and the animations played in the latest batch.
#[derive(Default)]
pub struct Shown {
    by_slot: BTreeMap<u16, Held>,
    own_pose: Option<u16>,
    tick: Option<u32>,
    played: Vec<(Option<u16>, u16)>,
    told: [u64; 3],
    drops: Drops,
}

const STATE: usize = 0;
const PLAYED: usize = 1;
const HELD: usize = 2;

impl Shown {
    pub fn dropping(drops: Drops) -> Self {
        Self {
            drops,
            ..Self::default()
        }
    }

    pub fn take(&mut self, frame: &[u8]) {
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&frame[LEN_BYTES..]) else {
            return;
        };
        self.tick = Some(batch.tick);
        self.played.clear();
        for record in batch.flatten() {
            match record {
                Record::Appear { slot, id, .. } => {
                    self.by_slot.insert(
                        slot,
                        Held {
                            id,
                            ..Held::default()
                        },
                    );
                }
                Record::Vanish { slot } => {
                    self.by_slot.remove(&slot);
                }
                Record::Game { slot, state } => {
                    if self.dropped(STATE) {
                        continue;
                    }
                    if let Some(held) = self.by_slot.get_mut(&slot) {
                        held.state = Some(state.to_vec());
                    }
                }
                Record::Show {
                    slot,
                    show: Show::Play(anim),
                } => {
                    if !self.dropped(PLAYED) {
                        self.played.push((slot, anim));
                    }
                }
                Record::Show {
                    slot,
                    show: Show::Hold(pose),
                } => {
                    if self.dropped(HELD) {
                        continue;
                    }
                    match slot {
                        None => self.own_pose = pose,
                        Some(slot) => {
                            if let Some(held) = self.by_slot.get_mut(&slot) {
                                held.pose = pose;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn dropped(&mut self, what: usize) -> bool {
        self.told[what] += 1;
        let drop = [self.drops.state, self.drops.played, self.drops.held][what];
        drop.is_some_and(|d| d.get() == self.told[what])
    }

    /// How many animations played once, and how many poses held or let go, it was told.
    pub fn told(&self) -> (u64, u64) {
        (self.told[PLAYED], self.told[HELD])
    }

    /// Where this model parts from what the server shows after `tick`: the entities in view and
    /// their state and pose, the bot's own pose, and each animation played in the tick, by slot.
    pub fn differs(
        &self,
        tick: u32,
        view: &[InView<'_>],
        own_pose: Option<u16>,
        played: &[(Option<u16>, u16)],
    ) -> Option<String> {
        let mut ours = self.by_slot.iter();
        for &InView {
            slot,
            id,
            state,
            pose,
        } in view
        {
            match ours.next() {
                Some((&s, held))
                    if s == slot
                        && held.id == id
                        && held.state.as_deref() == Some(state)
                        && held.pose == pose => {}
                Some((&s, held)) => {
                    return Some(format!(
                        "slot {s} holds {} as {:?} posed {:?}, and the server shows {id} there as \
                         {state:?} posed {pose:?}",
                        held.id, held.state, held.pose
                    ));
                }
                None => {
                    return Some(format!(
                        "slot {slot} holds nothing, and the server shows {id}"
                    ));
                }
            }
        }
        if let Some((s, held)) = ours.next() {
            return Some(format!(
                "slot {s} holds {}, which the server does not show",
                held.id
            ));
        }
        if self.own_pose != own_pose {
            return Some(format!(
                "its own body is posed {:?}, and the server holds it {own_pose:?}",
                self.own_pose
            ));
        }
        let ours: &[(Option<u16>, u16)] = if self.tick == Some(tick) {
            &self.played
        } else {
            &[]
        };
        let at = ours.iter().zip(played).take_while(|(a, b)| a == b).count();
        let (told, meant) = (ours.get(at), played.get(at));
        (told != meant).then(|| {
            format!(
                "its animation {at} of the tick was told as {told:?}, and the server played \
                 {meant:?} (by slot, and anim)"
            )
        })
    }
}
