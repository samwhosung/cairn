use std::collections::BTreeMap;

use protocol::{LEN_BYTES, Record, ServerMessage};

/// What a bot was sent of the game's state of each entity in its view, frame by frame as the
/// server sends them, before the network's delay.
#[derive(Default)]
pub struct Shown {
    by_slot: BTreeMap<u16, (u32, Option<Vec<u8>>)>,
    records: u64,
    drop: Option<u64>,
}

impl Shown {
    /// A model that leaves out the `drop`th state it is sent: a control for the check.
    pub fn dropping(drop: Option<u64>) -> Self {
        Self {
            drop,
            ..Self::default()
        }
    }

    pub fn take(&mut self, frame: &[u8]) {
        let Ok(ServerMessage::Batch(batch)) = ServerMessage::read(&frame[LEN_BYTES..]) else {
            return;
        };
        for record in batch.flatten() {
            match record {
                Record::Appear { slot, id, .. } => {
                    self.by_slot.insert(slot, (id, None));
                }
                Record::Vanish { slot } => {
                    self.by_slot.remove(&slot);
                }
                Record::Game { slot, state } => {
                    self.records += 1;
                    if self.drop == Some(self.records) {
                        continue;
                    }
                    if let Some(held) = self.by_slot.get_mut(&slot) {
                        held.1 = Some(state.to_vec());
                    }
                }
                _ => {}
            }
        }
    }

    /// The first way this differs from `view`, the server's: each entity in view by slot, and the
    /// state it now shows.
    pub fn differs(&self, view: &[(u16, u32, &[u8])]) -> Option<String> {
        let mut ours = self.by_slot.iter();
        for &(slot, id, state) in view {
            match ours.next() {
                Some((&s, (i, held)))
                    if s == slot && *i == id && held.as_deref() == Some(state) => {}
                Some((&s, (i, held))) => {
                    return Some(format!(
                        "slot {s} holds {i} as {held:?}, and the server shows {id} there as {state:?}"
                    ));
                }
                None => {
                    return Some(format!(
                        "slot {slot} holds nothing, and the server shows {id}"
                    ));
                }
            }
        }
        ours.next()
            .map(|(s, (i, _))| format!("slot {s} holds {i}, which the server does not show"))
    }
}
