use std::collections::BTreeMap;

use std::num::NonZeroU64;

use protocol::{LEN_BYTES, Record, ServerMessage};
use server::InView;

#[derive(Default)]
pub struct Shown {
    by_slot: BTreeMap<u16, (u32, Option<Vec<u8>>)>,
    records: u64,
    drop: Option<NonZeroU64>,
}

impl Shown {
    pub fn dropping(drop: Option<NonZeroU64>) -> Self {
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
                    if self.drop.is_some_and(|d| d.get() == self.records) {
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

    pub fn differs(&self, view: &[InView<'_>]) -> Option<String> {
        let mut ours = self.by_slot.iter();
        for &InView { slot, id, state } in view {
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
