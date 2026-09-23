use super::*;

fn worn(regions: [Option<&str>; 8], geoset_groups: [u32; 3]) -> ItemDisplay {
    ItemDisplay {
        region_textures: regions.map(|r| r.map(str::to_string)),
        geoset_groups,
        ..ItemDisplay::default()
    }
}

fn dressed<'a>(slots: &[(usize, &'a ItemDisplay)]) -> [Option<&'a ItemDisplay>; 8] {
    let mut e = [None; 8];
    for &(i, d) in slots {
        e[i] = Some(d);
    }
    e
}

fn cells(plan: &[EquipBlit<'_>]) -> Vec<(usize, i8, String)> {
    plan.iter()
        .map(|s| {
            let what = match s.source {
                BlitSource::Worn { texture, .. } => texture.to_string(),
                BlitSource::Emblem { part, .. } => format!("{part:?}"),
            };
            (s.layer, s.column, what)
        })
        .collect()
}

fn layer(plan: &[EquipBlit<'_>], layer: usize) -> Vec<(i8, String)> {
    cells(plan)
        .into_iter()
        .filter(|c| c.0 == layer)
        .map(|(_, c, t)| (c, t))
        .collect()
}

fn want(v: &[(i8, &str)]) -> Vec<(i8, String)> {
    v.iter().map(|&(c, t)| (c, t.to_string())).collect()
}

const ARMS: [Option<&str>; 8] = [
    Some("au"),
    Some("al"),
    None,
    Some("tu"),
    None,
    None,
    None,
    None,
];
const TORSO: [Option<&str>; 8] = [
    None,
    None,
    None,
    Some("tabard_tu"),
    Some("tabard_tl"),
    None,
    None,
    None,
];
const EMBLEM: GuildEmblem = GuildEmblem {
    emblem_style: 42,
    emblem_color: 3,
    border_style: 1,
    border_color: 7,
    background_color: 12,
};

#[test]
fn a_sleeved_chest_and_geoset_gloves_paint_over_a_bracer() {
    let bracer = worn(
        [None, Some("bracer_al"), None, None, None, None, None, None],
        [0; 3],
    );
    let gloves = |g| {
        worn(
            [
                None,
                Some("glove_al"),
                Some("glove_ha"),
                None,
                None,
                None,
                None,
                None,
            ],
            [g, 0, 0],
        )
    };
    let (plain_chest, sleeved_chest) = (worn(ARMS, [0, 0, 0]), worn(ARMS, [1, 0, 0]));
    let (plain_gloves, geoset_gloves) = (gloves(0), gloves(1));
    let eq = dressed(&[(1, &plain_chest), (5, &bracer), (6, &plain_gloves)]);
    assert_eq!(
        layer(&equip_blits(&eq, None, false), 1),
        want(&[(1, "al"), (2, "bracer_al"), (3, "glove_al")])
    );
    let eq = dressed(&[(1, &sleeved_chest), (5, &bracer), (6, &geoset_gloves)]);
    assert_eq!(
        layer(&equip_blits(&eq, None, false), 1),
        want(&[(2, "bracer_al"), (5, "al"), (6, "glove_al")])
    );
    assert!(forearm_dressed(&eq));
    let shirt = worn(ARMS, [0; 3]);
    assert!(
        !forearm_dressed(&dressed(&[(0, &shirt)])),
        "the shirt alone"
    );
}

#[test]
fn a_guild_emblem_replaces_the_tabard_that_takes_it() {
    let guild_tabard = ItemDisplay {
        flags: 1,
        ..worn(TORSO, [1, 0, 0])
    };
    let plain_tabard = worn(TORSO, [1, 0, 0]);
    let shirt = worn(
        [
            Some("s_au"),
            Some("s_al"),
            None,
            Some("s_tu"),
            Some("s_tl"),
            None,
            None,
            None,
        ],
        [0; 3],
    );
    let crest = |l| {
        [(l, 2, "Background"), (l, 3, "Border"), (l, 4, "Symbol")]
            .map(|(l, c, t): (usize, i8, &str)| (l, c, t.to_string()))
    };
    let eq = dressed(&[(7, &guild_tabard)]);
    assert_eq!(
        cells(&equip_blits(&eq, Some(EMBLEM), false)),
        [crest(3), crest(4)].concat()
    );
    let own = vec![
        (3, 4, "tabard_tu".to_string()),
        (4, 4, "tabard_tl".to_string()),
    ];
    assert_eq!(cells(&equip_blits(&eq, None, false)), own);
    let plain = dressed(&[(7, &plain_tabard)]);
    assert_eq!(cells(&equip_blits(&plain, Some(EMBLEM), false)), own);
    assert!(equip_blits(&[None; 8], Some(EMBLEM), false).is_empty());
    assert_eq!(
        cells(&equip_blits(&[None; 8], Some(EMBLEM), true)),
        [crest(3), crest(4)].concat(),
        "the designer paints over an empty slot"
    );
    let eq = dressed(&[(0, &shirt), (7, &guild_tabard)]);
    let mut expect = vec![(0, 0, "s_au".to_string()), (1, 0, "s_al".to_string())];
    expect.push((3, 0, "s_tu".to_string()));
    expect.extend(crest(3));
    expect.push((4, 0, "s_tl".to_string()));
    expect.extend(crest(4));
    assert_eq!(cells(&equip_blits(&eq, Some(EMBLEM), false)), expect);
}

#[test]
fn only_a_guild_tabards_background_covers_the_bra() {
    let guild_tabard = ItemDisplay {
        flags: 1,
        ..worn(TORSO, [1, 0, 0])
    };
    let plain_tabard = worn(TORSO, [1, 0, 0]);
    let bra = &UNDERWEAR[1];
    assert_eq!((bra.texture_column, bra.layer), (1, 3));
    let guild = dressed(&[(7, &guild_tabard)]);
    let plain = dressed(&[(7, &plain_tabard)]);
    assert!(bra.covered(&equip_blits(&guild, Some(GuildEmblem::default()), false)));
    assert!(!bra.covered(&equip_blits(&guild, None, false)));
    assert!(!bra.covered(&equip_blits(&plain, Some(GuildEmblem::default()), false)));
}

#[test]
fn a_robe_paints_over_footwear_on_the_lower_leg() {
    let leg = |t| [None, None, None, None, None, Some("lu"), Some(t), None];
    let robe = worn(leg("robe_ll"), [1, 0, 1]);
    let plain_chest = worn(leg("chest_ll"), [0, 0, 0]);
    let trousers = worn(leg("pant_ll"), [0, 0, 0]);
    let robe_trousers = worn(leg("robetrouser_ll"), [0, 0, 1]);
    let feet = |g| {
        worn(
            [
                None,
                None,
                None,
                None,
                None,
                None,
                Some("boot_ll"),
                Some("boot_foot"),
            ],
            [g, 0, 0],
        )
    };
    let (shoes, boots) = (feet(0), feet(3));
    let g6 = |eq: &[Option<&ItemDisplay>; 8]| layer(&equip_blits(eq, None, false), 6);
    assert_eq!(
        g6(&dressed(&[(1, &robe), (4, &shoes)])),
        want(&[(2, "boot_ll"), (4, "robe_ll")])
    );
    assert_eq!(
        g6(&dressed(&[(1, &robe), (4, &boots)])),
        want(&[(3, "boot_ll"), (4, "robe_ll")])
    );
    assert_eq!(
        g6(&dressed(&[(1, &plain_chest), (3, &trousers), (4, &boots)])),
        want(&[(0, "pant_ll"), (1, "chest_ll"), (3, "boot_ll")])
    );
    assert_eq!(
        g6(&dressed(&[(1, &robe), (3, &robe_trousers)])),
        want(&[(3, "robetrouser_ll"), (4, "robe_ll")])
    );
    assert_eq!(
        g6(&dressed(&[(1, &plain_chest), (3, &robe_trousers)])),
        want(&[(1, "chest_ll"), (4, "robetrouser_ll")])
    );
    assert_eq!(
        g6(&dressed(&[(1, &robe), (3, &robe_trousers), (4, &boots)])),
        want(&[(3, "boot_ll"), (4, "robe_ll")]),
        "one cell holds one paint, the later slot's"
    );
}

#[test]
fn unreached_layers_and_empty_names_paint_nothing() {
    let odd = worn(
        [None, None, None, Some("boot_tu"), None, None, None, None],
        [0; 3],
    );
    assert!(equip_blits(&dressed(&[(4, &odd)]), None, false).is_empty());
    let blank = worn([None, None, None, Some(""), None, None, None, None], [0; 3]);
    assert!(equip_blits(&dressed(&[(1, &blank)]), None, false).is_empty());
}

#[test]
fn region_textures_try_unisex_before_the_gender_letter() {
    assert_eq!(
        equip_region_candidates(6, "Leather_A_02_Pant_LL", 1),
        [
            "Item\\TextureComponents\\LegLowerTexture\\Leather_A_02_Pant_LL_U.blp",
            "Item\\TextureComponents\\LegLowerTexture\\Leather_A_02_Pant_LL_F.blp",
        ]
    );
    let male = equip_region_candidates(6, "Leather_A_02_Pant_LL", 0);
    assert!(male[0].ends_with("_U.blp") && male[1].ends_with("_M.blp"));
    assert_eq!(equip_tile(7), Some((128, 224, 128, 32)));
    assert_eq!((equip_tile(8), equip_tex_dir(8)), (None, None));
}
