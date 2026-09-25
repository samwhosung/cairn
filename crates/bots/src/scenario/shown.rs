use std::collections::BTreeMap;

use std::num::NonZeroU64;

use protocol::{LEN_BYTES, Record, ServerMessage, Show, Whose};
use server::{Attacked, InView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Nth(pub NonZeroU64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drops {
    pub state: Option<Nth>,
    pub play: Option<Nth>,
    pub hold: Option<Nth>,
    pub idle: Option<Nth>,
    pub attack: Option<Nth>,
}

#[derive(Default)]
struct Held {
    id: u32,
    state: Option<Vec<u8>>,
    pose: Option<u16>,
    idle: Option<u16>,
}

#[derive(Default)]
pub struct Shown {
    by_slot: BTreeMap<u16, Held>,
    own_pose: Option<u16>,
    own_idle: Option<u16>,
    tick: Option<u32>,
    latest_plays: Vec<(Whose, u16)>,
    latest_attacks: Vec<Attacked>,
    told: [u64; 5],
    drops: Drops,
}

const STATE: usize = 0;
const PLAYED: usize = 1;
const HELD: usize = 2;
const IDLED: usize = 3;
const ATTACKED: usize = 4;

pub struct ServerOwnShows {
    pub pose: Option<u16>,
    pub idle: Option<u16>,
}

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
        self.latest_plays.clear();
        self.latest_attacks.clear();
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
                    whose,
                    show: Show::Play(anim),
                } => {
                    if !self.dropped(PLAYED) {
                        self.latest_plays.push((whose, anim));
                    }
                }
                Record::Show {
                    whose,
                    show: Show::Attack { target, outcome },
                } => {
                    if !self.dropped(ATTACKED) {
                        self.latest_attacks.push(Attacked {
                            attacker: whose,
                            target,
                            outcome,
                        });
                    }
                }
                Record::Show {
                    whose,
                    show: Show::Hold(pose),
                } => {
                    if self.dropped(HELD) {
                        continue;
                    }
                    match whose {
                        Whose::Own => self.own_pose = pose,
                        Whose::Slot(slot) => {
                            if let Some(held) = self.by_slot.get_mut(&slot) {
                                held.pose = pose;
                            }
                        }
                    }
                }
                Record::Show {
                    whose,
                    show: Show::Idle(idle),
                } => {
                    if self.dropped(IDLED) {
                        continue;
                    }
                    match whose {
                        Whose::Own => self.own_idle = idle,
                        Whose::Slot(slot) => {
                            if let Some(held) = self.by_slot.get_mut(&slot) {
                                held.idle = idle;
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
        let drop = [
            self.drops.state,
            self.drops.play,
            self.drops.hold,
            self.drops.idle,
            self.drops.attack,
        ][what];
        drop.is_some_and(|Nth(n)| n.get() == self.told[what])
    }

    pub fn plays_told(&self) -> u64 {
        self.told[PLAYED]
    }

    pub fn poses_told(&self) -> u64 {
        self.told[HELD]
    }

    pub fn idles_told(&self) -> u64 {
        self.told[IDLED]
    }

    pub fn attacks_told(&self) -> u64 {
        self.told[ATTACKED]
    }

    pub fn first_difference(
        &self,
        tick: u32,
        view: &[InView<'_>],
        own: &ServerOwnShows,
        played: &[(Whose, u16)],
        attacked: &[Attacked],
    ) -> Option<String> {
        let mut ours = self.by_slot.iter();
        for &InView {
            slot,
            id,
            state,
            pose,
            idle,
        } in view
        {
            match ours.next() {
                Some((&s, held))
                    if s == slot
                        && held.id == id
                        && held.state.as_deref() == Some(state)
                        && held.pose == pose
                        && held.idle == idle => {}
                Some((&s, held)) => {
                    return Some(format!(
                        "slot {s} holds {} as {:?} posed {:?} idling {:?}, and the server shows \
                         {id} there as {state:?} posed {pose:?} idling {idle:?}",
                        held.id, held.state, held.pose, held.idle
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
        if self.own_pose != own.pose {
            return Some(format!(
                "its own body is posed {:?}, and the server holds it {:?}",
                self.own_pose, own.pose
            ));
        }
        if self.own_idle != own.idle {
            return Some(format!(
                "its own body idles in {:?}, and the server idles it in {:?}",
                self.own_idle, own.idle
            ));
        }
        let this_tick = self.tick == Some(tick);
        let plays: &[(Whose, u16)] = if this_tick { &self.latest_plays } else { &[] };
        let attacks: &[Attacked] = if this_tick { &self.latest_attacks } else { &[] };
        first_told_apart("animation", plays, played)
            .or_else(|| first_told_apart("attack", attacks, attacked))
    }
}

fn first_told_apart<T: PartialEq + std::fmt::Debug>(
    what: &str,
    told: &[T],
    meant: &[T],
) -> Option<String> {
    let at = told.iter().zip(meant).take_while(|(a, b)| a == b).count();
    let (told, meant) = (told.get(at), meant.get(at));
    (told != meant).then(|| {
        format!("its {what} {at} of the tick was told as {told:?}, and the server made {meant:?}")
    })
}
