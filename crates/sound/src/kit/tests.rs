use super::voice::{Bus, VoiceSlot, bus_at_cap, no_duplicates_blocks, pick_voice_slot};
use super::voice::{SOFTWARE_CHANNELS, same_kit_cap_blocks};
use super::*;

#[test]
fn the_pool_plays_every_variation_once_a_cycle() {
    let mut kits = SoundKits::empty();
    let weights = [1u32, 1, 1, 1, 1];
    for _ in 0..3 {
        let mut seen = [false; 5];
        for _ in 0..5 {
            let i = kits.pick_variation(7, &weights);
            assert!(!seen[i], "a repeat within the cycle");
            seen[i] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }
}

#[test]
fn a_bus_refuses_at_its_own_cap() {
    let live = |bus: u8, n: usize| std::iter::repeat_n(Bus(bus), n);
    assert!(!bus_at_cap(live(0, 1000), Bus::DEFAULT));
    assert!(bus_at_cap(live(5, 1), Bus(5)));
    assert!(!bus_at_cap(live(9, 5), Bus::FOOTSTEP));
    assert!(bus_at_cap(live(9, 6), Bus::FOOTSTEP));
    assert!(!bus_at_cap(live(5, 9), Bus(7)));
}

#[test]
fn the_ceiling_steals_the_quietest_one_shot_only_for_a_louder_one() {
    assert_eq!(
        pick_voice_slot([(0, 0.9f32)].into_iter(), SOFTWARE_CHANNELS - 1, 0.001),
        VoiceSlot::Free
    );
    let live = [(0, 0.50f32), (1, 0.30), (2, 0.05), (3, 0.40)];
    assert_eq!(
        pick_voice_slot(live.into_iter(), SOFTWARE_CHANNELS, 0.9),
        VoiceSlot::Steal(2)
    );
    let twins: Vec<(usize, f32)> = (0..12).map(|i| (i, 0.7)).collect();
    assert_eq!(
        pick_voice_slot(twins.into_iter(), SOFTWARE_CHANNELS, 0.7),
        VoiceSlot::Denied
    );
    assert_eq!(
        pick_voice_slot(std::iter::empty(), SOFTWARE_CHANNELS, 1.0),
        VoiceSlot::Denied
    );
}

#[test]
fn the_duplicate_gates_belong_to_the_one_shot_lane() {
    const LAMP: u32 = 0x220;
    assert!(no_duplicates_blocks(false, LAMP, 1));
    assert!(!no_duplicates_blocks(true, LAMP, 1));
    assert!(!same_kit_cap_blocks(false, 1));
    assert!(same_kit_cap_blocks(false, 2));
    assert!(!same_kit_cap_blocks(true, 8));
}
