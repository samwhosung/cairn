use super::*;

#[test]
fn a_chance_passes_threshold_and_one_in_a_hundred_and_one() {
    let passing = |threshold| {
        (0..101u64)
            .map(|bucket| ((bucket << 32) / 101 + 1) as u32)
            .filter(|&roll| chance_passes(threshold, roll))
            .count()
    };
    assert_eq!(passing(EXERTION_CHANCE_PLAYER), 36);
    assert_eq!(passing(INJURY_CHANCE_PLAYER), 31);
    assert_eq!(passing(EXERTION_CHANCE_CREATURE), 71);
    assert_eq!(passing(100), 101);
}

#[test]
fn an_outcome_whiffs_lands_is_defended_or_absorbed_as_the_client_sounds_it() {
    use Outcome::{Absorb, Block, Crit, Crushing, Dodge, Hit, Immune, Miss, Parry};
    let all = [
        Hit, Crit, Crushing, Miss, Dodge, Parry, Block, Absorb, Immune,
    ];
    let which =
        |f: fn(Outcome) -> bool| -> Vec<Outcome> { all.into_iter().filter(|&o| f(o)).collect() };
    assert_eq!(which(whiffs), [Miss, Dodge]);
    assert_eq!(which(grunts).len(), all.len() - 1, "all but a miss");
    assert_eq!(
        which(makes_contact),
        [Hit, Crit, Crushing, Parry, Block, Absorb]
    );
    assert_eq!(which(defended), [Parry, Block]);
    assert_eq!(which(absorbed), [Absorb, Immune]);
    assert_eq!(which(wounds), [Hit, Crit, Crushing]);
}

#[test]
fn a_creature_strikes_as_flesh_stone_wood_or_ethereal_and_past_that_as_flesh() {
    assert_eq!(
        [0, 1, 2, 3, 4, 99].map(creature_impact_slot),
        [
            impact_slot::FLESH,
            impact_slot::STONE,
            impact_slot::WOOD,
            impact_slot::ETHEREAL,
            impact_slot::FLESH,
            impact_slot::FLESH
        ]
    );
    assert_eq!(blow_key(*b"$CAH"), Some(Blow::Weapon));
    assert_eq!(blow_key(*b"$AH2"), Some(Blow::Natural(2)));
    assert_eq!(blow_key(*b"$HIT"), None, "an inert key");
    assert_eq!(blow_key(*b"$CSS"), None);
}

mod heard;
