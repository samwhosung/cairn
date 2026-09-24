use super::mixer;
use crate::SoundOutput;

/// A voice bus: the client's concurrency domain, which a play at its bus's cap is refused on
/// outright. Orthogonal to the category a channel's volume follows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Bus(pub u8);

impl Bus {
    /// Uncapped: the bus of every play that names none.
    pub const DEFAULT: Bus = Bus(0);
    /// The terrain step of a footfall, six at once.
    pub const FOOTSTEP: Bus = Bus(9);
}

/// The client's per-bus caps, a table no code writes.
const BUS_CAP: [u32; 13] = [0x7fff_ffff, 1, 2, 2, 1, 1, 2, 2, 1, 6, 4, 1, 2];

/// How many live copies of a kit without the no-duplicates flag may play: sample-aligned copies
/// sum coherently, so a third is louder, not denser. The client caps only flagged kits, at one.
const SAME_KIT_MAX: usize = 2;

/// The client's device ceiling: twelve voices, music and ambience included.
pub(crate) const SOFTWARE_CHANNELS: usize = 12;

pub(super) fn no_duplicates_blocks(dedupe_exempt: bool, flags: u32, live_same_kit: usize) -> bool {
    !dedupe_exempt && flags & crate::tables::kit_flags::NO_DUPLICATES != 0 && live_same_kit > 0
}

pub(super) fn same_kit_cap_blocks(dedupe_exempt: bool, live_same_kit: usize) -> bool {
    !dedupe_exempt && live_same_kit >= SAME_KIT_MAX
}

/// Counts allocated channels, not audible ones.
pub(super) fn bus_at_cap(live: impl Iterator<Item = Bus>, bus: Bus) -> bool {
    let cap = BUS_CAP[usize::from(bus.0)];
    cap != BUS_CAP[0] && live.filter(|b| *b == bus).count() as u32 >= cap
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum VoiceSlot {
    Free,
    Steal(usize),
    Denied,
}

/// The client drops the newcomer every time; stealing keeps the loudest one-shots.
pub(super) fn pick_voice_slot(
    stealable: impl Iterator<Item = (usize, f32)>,
    live_voices: usize,
    candidate_amp: f32,
) -> VoiceSlot {
    if live_voices < SOFTWARE_CHANNELS {
        return VoiceSlot::Free;
    }
    match stealable.min_by(|a, b| a.1.total_cmp(&b.1)) {
        Some((i, amp)) if amp < candidate_amp => VoiceSlot::Steal(i),
        _ => VoiceSlot::Denied,
    }
}

pub(super) fn claim_voice(out: &mut SoundOutput, candidate_amp: f32) -> bool {
    if out.live_voices() >= SOFTWARE_CHANNELS {
        out.channels
            .retain(|c| c.handle.state() != kira::sound::PlaybackState::Stopped);
    }
    let stealable = out
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.looping)
        .map(|(i, c)| (i, c.fed_amp));
    match pick_voice_slot(stealable, out.live_voices(), candidate_amp) {
        VoiceSlot::Free => true,
        VoiceSlot::Steal(i) => {
            out.channels[i].handle.stop(mixer::declick());
            out.channels.swap_remove(i);
            out.voices_stolen += 1;
            true
        }
        VoiceSlot::Denied => {
            out.voices_denied += 1;
            false
        }
    }
}
