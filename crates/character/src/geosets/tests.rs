use super::*;

fn tables(
    hair: &[(Key, u32)],
    facial: &[(Key, [u32; 3])],
    helmet_vis: &[(u32, [u32; 5])],
) -> CharacterGeosets {
    CharacterGeosets {
        hair: hair.iter().copied().collect(),
        facial: facial.iter().copied().collect(),
        helmet_vis: helmet_vis.iter().copied().collect(),
    }
}

fn worn(slot: usize, groups: [u32; 3]) -> EquipGeosets {
    let mut eq = EquipGeosets::default();
    eq.bodyslots[slot] = Some(groups);
    eq
}

#[test]
fn gloves_boots_robes_and_cloaks_replace_their_groups() {
    let cg = tables(&[], &[], &[]);
    let naked = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
    assert!(naked.contains(&401) && naked.contains(&1101) && naked.contains(&1501));

    let set = cg.visible_geosets(1, 0, 0, 0, &worn(6, [1, 0, 0]));
    assert!(set.contains(&402) && !set.contains(&401), "gloves");

    let set = cg.visible_geosets(1, 0, 0, 0, &worn(4, [2, 0, 0]));
    assert!(
        set.contains(&503) && !set.contains(&501),
        "boots take the bare foot off"
    );

    let mut eq = worn(1, [1, 0, 1]);
    eq.bodyslots[3] = Some([2, 1, 0]);
    eq.bodyslots[4] = Some([2, 0, 0]);
    eq.bodyslots[7] = Some([1, 0, 0]);
    let set = cg.visible_geosets(1, 0, 0, 0, &eq);
    assert!(set.contains(&1302), "robe skirt on");
    assert!(
        !set.contains(&1101) && !set.contains(&1301),
        "trouser legs off"
    );
    assert!(!set.contains(&501) && !set.contains(&503), "boot group off");
    assert!(!set.contains(&1202), "no tabard flap under a robe");
    assert!(set.contains(&802), "the chest's sleeves still show");

    let eq = EquipGeosets {
        cloak: Some(4),
        ..EquipGeosets::default()
    };
    let set = cg.visible_geosets(1, 0, 0, 0, &eq);
    assert!(set.contains(&1505) && !set.contains(&1501), "cloak");
}

#[test]
fn the_shirt_cuff_follows_the_forearm_not_the_chest_slot() {
    let cg = tables(&[], &[], &[]);
    let shirt = |dressed: bool| EquipGeosets {
        forearm_dressed: dressed,
        ..worn(0, [1, 0, 0])
    };
    assert!(cg.visible_geosets(1, 0, 0, 0, &shirt(false)).contains(&802));
    assert!(!cg.visible_geosets(1, 0, 0, 0, &shirt(true)).contains(&802));
    let mut eq = shirt(false);
    eq.bodyslots[1] = Some([0, 0, 0]);
    assert!(
        cg.visible_geosets(1, 0, 0, 0, &eq).contains(&802),
        "a chest that leaves the forearm bare keeps the cuff"
    );
}

#[test]
fn the_tabard_designer_wears_the_flap_over_an_empty_slot() {
    let cg = tables(&[], &[], &[]);
    let set = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
    assert!(!set.contains(&1202));
    let preview = EquipGeosets {
        tabard_preview: true,
        ..EquipGeosets::default()
    };
    let set = cg.visible_geosets(1, 0, 0, 0, &preview);
    assert!(set.contains(&1202) && set.contains(&1201));
    for slot in [1usize, 3] {
        let mut eq = preview;
        eq.bodyslots[slot] = Some([0, 0, 1]);
        assert!(
            !cg.visible_geosets(1, 0, 0, 0, &eq).contains(&1202),
            "a robe in slot {slot} hides the flap"
        );
    }
}

#[test]
fn the_doublet_and_trouser_legs_yield_to_a_chest_robe_or_a_tabard() {
    let cg = tables(&[], &[], &[]);
    let mut base = worn(0, [0, 1, 0]);
    base.bodyslots[3] = Some([1, 0, 0]);
    let set = cg.visible_geosets(1, 0, 0, 0, &base);
    assert!(set.contains(&1002) && set.contains(&1103));

    let mut eq = base;
    eq.bodyslots[7] = Some([1, 0, 0]);
    let set = cg.visible_geosets(1, 0, 0, 0, &eq);
    assert!(set.contains(&1202));
    assert!(!set.contains(&1002) && !set.contains(&1103), "a tabard");

    let mut eq = base;
    eq.bodyslots[1] = Some([0, 0, 1]);
    let set = cg.visible_geosets(1, 0, 0, 0, &eq);
    assert!(!set.contains(&1002) && !set.contains(&1103), "a chest robe");

    let mut eq = base;
    eq.bodyslots[3] = Some([1, 0, 1]);
    let set = cg.visible_geosets(1, 0, 0, 0, &eq);
    assert!(
        set.contains(&1002) && set.contains(&1103),
        "a legs robe does not"
    );
}

#[test]
fn a_helm_puts_the_masked_groups_back_to_their_bases() {
    let cg = tables(
        &[((1, 0, 1), 5), ((1, 1, 1), 5)],
        &[((1, 0, 1), [2, 4, 3])],
        &[(368, [u32::MAX; 5]), (245, [0; 5]), (300, [1 << 2; 5])],
    );
    let helm = |rows| EquipGeosets {
        helm_vis: Some(rows),
        ..EquipGeosets::default()
    };
    let set = cg.visible_geosets(1, 0, 1, 1, &helm([368, 245]));
    assert!(set.contains(&1) && !set.contains(&5), "hair to the scalp");
    assert!(set.contains(&101) && !set.contains(&102));
    assert!(set.contains(&201) && !set.contains(&204));
    assert!(set.contains(&301) && !set.contains(&303));
    assert!(set.contains(&701) && !set.contains(&702), "ears to 701");
    let set = cg.visible_geosets(1, 1, 1, 1, &helm([368, 245]));
    assert!(
        set.contains(&5) && set.contains(&702),
        "the female row hides nothing"
    );
    let set = cg.visible_geosets(1, 1, 1, 1, &helm([300, 300]));
    assert!(set.contains(&5), "a mask without the race's bit");
}

#[test]
fn an_appearance_selects_its_hair_and_facial_geosets() {
    let cg = tables(
        &[((1, 0, 1), 2), ((1, 0, 0), 0)],
        &[((1, 0, 1), [1, 1, 2])],
        &[],
    );
    let set = cg.visible_geosets(1, 0, 1, 1, &EquipGeosets::default());
    for id in [0, 2, 101, 302, 201, 401, 702, 1501] {
        assert!(set.contains(&id), "{id} in {set:?}");
    }
    assert!(!set.contains(&5));
    let bald = cg.visible_geosets(1, 0, 0, 0, &EquipGeosets::default());
    assert!(bald.contains(&1), "a bald style shows the scalp");
    assert!(set.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
}
