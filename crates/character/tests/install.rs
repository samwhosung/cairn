use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::OnceLock;

use character::{
    CharCreateCatalog, CharSections, CharacterGeosets, CreatureCatalog, EmblemLayer, EquipGeosets,
    GuildEmblem, ItemDisplay, ItemDisplayCatalog, MipChain, equip_blits, equip_tile,
    read_mip_chain,
};

fn chain() -> Option<&'static mpq::Chain> {
    static CHAIN: OnceLock<Option<mpq::Chain>> = OnceLock::new();
    CHAIN
        .get_or_init(|| {
            let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
                eprintln!("skipped: WOW_DATA is not set");
                return None;
            };
            Some(mpq::Chain::open(data).expect("open the chain"))
        })
        .as_ref()
}

fn raw_table(chain: &mpq::Chain, name: &str, columns: &str) -> Vec<Vec<String>> {
    let bytes = chain
        .read(&format!("DBFilesClient\\{name}.dbc"))
        .expect(name);
    let mut schema = dbc::Schema::new(name);
    for c in columns.chars() {
        let ty = if c == 's' {
            dbc::FieldType::String
        } else {
            dbc::FieldType::UInt32
        };
        schema.add_field(dbc::SchemaField::new("", ty));
    }
    let rs = dbc::DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
        .and_then(|p| p.with_schema(schema))
        .and_then(|p| p.parse_records())
        .expect(name);
    rs.records()
        .iter()
        .map(|r| {
            (0..columns.len())
                .map(|i| match r.get_value(i) {
                    Some(dbc::Value::UInt32(v)) => v.to_string(),
                    Some(dbc::Value::StringRef(at)) => {
                        rs.get_string(*at).expect("string").into_owned()
                    }
                    other => panic!("{name} column {i}: {other:?}"),
                })
                .collect()
        })
        .collect()
}

fn num(row: &[String], i: usize) -> u8 {
    row[i].parse().expect("a number")
}

type Tile = (u32, u32, u32, u32);
const HEAD_UPPER: Tile = (0, 160, 128, 32);
const HEAD_LOWER: Tile = (0, 192, 128, 64);

fn tile(layer: usize) -> Tile {
    equip_tile(layer).expect("an equipment layer")
}

fn rect(img: &MipChain, (x, y, w, h): Tile) -> Vec<u8> {
    (0..h)
        .flat_map(|r| {
            let o = (((y + r) * img.width + x) * 4) as usize;
            img.mips[0][o..o + (w * 4) as usize].to_vec()
        })
        .collect()
}

fn texels_differing(a: &[u8], b: &[u8]) -> usize {
    a.chunks(4).zip(b.chunks(4)).filter(|(x, y)| x != y).count()
}

#[allow(clippy::too_many_arguments)]
fn compose(
    cs: &CharSections,
    chain: &mpq::Chain,
    (race, sex): (u8, u8),
    [skin, face, facial_hair, hair_style, hair_color]: [u8; 5],
    equipment: [Option<&ItemDisplay>; 8],
    emblem: Option<GuildEmblem>,
) -> MipChain {
    cs.composite_body(
        chain,
        race,
        sex,
        skin,
        face,
        facial_hair,
        hair_style,
        hair_color,
        equipment,
        emblem,
        false,
    )
    .expect("composite")
    .expect("a skin row")
}

#[test]
fn character_creation_matches_the_shipped_tables() {
    let Some(chain) = chain() else { return };
    let cat = CharCreateCatalog::load(chain).expect("load the catalog");
    let creatures = CreatureCatalog::load(chain).expect("load the creatures");
    for race in 1..=8u8 {
        for sex in 0..=1u8 {
            let display = cat.body_display(race, sex).expect("a body display");
            let model = creatures.model(display).expect("the display resolves");
            assert!(
                model
                    .model_path
                    .to_ascii_lowercase()
                    .starts_with("character\\"),
                "race {race} sex {sex}: {}",
                model.model_path
            );
        }
    }
    assert_eq!(cat.classes_for_race(1), [1, 2, 4, 5, 8, 9]);
    assert_eq!(cat.classes_for_race(3), [1, 2, 3, 4, 5], "no Dwarf Mage");
    assert!(!cat.allows(3, 8) && cat.allows(3, 5));
    assert_eq!(cat.hair_customization(6), Some("HORNS"));
    assert_eq!(cat.hair_customization(1), Some("NORMAL"));
    assert_eq!(cat.facial_hair_customization(1, 0), Some("NORMAL"));
    assert_eq!(cat.facial_hair_customization(1, 1), Some("PIERCINGS"));
    assert_eq!(cat.facial_hair_customization(4, 1), Some("MARKINGS"));
    assert_eq!(cat.facial_hair_customization(8, 0), Some("TUSKS"));
    assert_eq!(cat.race_file(5), Some("Scourge"));
    for race in 1..=8u8 {
        for class in cat.classes_for_race(race) {
            for sex in 0..=1u8 {
                let outfit = cat.start_outfit(race, class, sex);
                assert!(!outfit.is_empty(), "race {race} class {class} sex {sex}");
                assert!(outfit.iter().all(|i| i.display_id >= 1 && i.inv_type >= 1));
            }
        }
    }
    assert!(
        cat.start_outfit(1, 8, 0)
            .iter()
            .any(|i| i.inv_type == 20 && i.display_id == 12647),
        "a Human Mage starts in a robe"
    );
}

#[test]
fn every_dial_combination_names_selectable_rows() {
    let Some(chain) = chain() else { return };
    let cat = CharCreateCatalog::load(chain).expect("load the catalog");
    let selectable: HashSet<[u8; 5]> = raw_table(chain, "CharSections", "uuuuuusssu")
        .iter()
        .filter(|r| r[9].parse::<u32>().expect("flags") & 1 == 0)
        .map(|r| [1, 2, 3, 4, 5].map(|i| num(r, i)))
        .collect();
    let facial: HashSet<[u8; 3]> = raw_table(chain, "CharacterFacialHairStyles", "uuuuuuuuu")
        .iter()
        .map(|r| [0, 1, 2].map(|i| num(r, i)))
        .collect();
    for race in 1..=8u8 {
        for sex in 0..=1u8 {
            let r = cat.ranges(race, sex).expect("ranges");
            let has = |ty, var, color| selectable.contains(&[race, sex, ty, var, color]);
            // Tauren, and every female but Night Elf and Undead, have no facial hair sections:
            // the dial picks geometry alone.
            let facial_sections = race != 6 && (sex == 0 || race == 4 || race == 5);
            for skin in 0..r.skin {
                assert!(has(0, 0, skin));
                for face in 0..r.face {
                    assert!(has(1, face, skin), "race {race} sex {sex} face {face}");
                }
            }
            for style in 0..r.hair_style {
                for color in 0..r.hair_color {
                    assert!(has(3, style, color), "race {race} sex {sex} hair {style}");
                    for beard in 0..r.facial_hair {
                        assert!(facial.contains(&[race, sex, beard]));
                        assert!(!facial_sections || has(2, beard, color));
                    }
                }
            }
        }
    }
}

#[test]
fn a_repeated_hair_key_takes_its_first_row() {
    let Some(chain) = chain() else { return };
    let mut keys: HashMap<[u8; 3], Vec<u8>> = HashMap::new();
    for r in raw_table(chain, "CharHairGeosets", "uuuuuu") {
        keys.entry([1, 2, 3].map(|i| num(&r, i)))
            .or_default()
            .push(num(&r, 4));
    }
    let repeated: Vec<_> = keys.iter().filter(|(_, v)| v.len() > 1).collect();
    assert_eq!(repeated, [(&[9, 0, 0], &vec![1, 2, 1, 2])]);
    let cg = CharacterGeosets::load(chain).expect("load the geoset tables");
    let male = cg.visible_geosets(9, 0, 0, 0, &EquipGeosets::default());
    assert!(male.contains(&1) && !male.contains(&2), "{male:?}");
    let female = cg.visible_geosets(9, 1, 0, 0, &EquipGeosets::default());
    assert!(female.contains(&1) && !female.contains(&2), "{female:?}");
}

#[test]
fn skins_hair_and_fur_resolve_their_rows() {
    let Some(chain) = chain() else { return };
    let cs = CharSections::load(chain).expect("load CharSections");
    assert_eq!(
        cs.skin_texture(1, 0, 0),
        Some("Character\\Human\\Male\\HumanMaleSkin00_00.blp"),
        "a selectable row wins over an unselectable one"
    );
    assert_eq!(
        cs.skin_extra_texture(6, 0, 0),
        Some("Character\\Tauren\\Male\\TaurenMaleSkin00_00_Extra.blp")
    );
    assert_eq!(cs.skin_extra_texture(1, 0, 0), None);
    assert!(
        cs.hair_texture(1, 0, 1, 0)
            .is_some_and(|h| h.contains("Hair"))
    );
    assert_eq!(cs.hair_texture(1, 0, 0, 0), None, "bald");
    for (race, name) in [(2u8, "Orc"), (7, "Gnome")] {
        let bald = cs.hair_mesh_texture(race, 0, 0, 0).expect(name);
        assert_eq!(Some(bald), cs.hair_mesh_texture(race, 0, 1, 0), "{name}");
        assert!(bald.contains("Hair00_00"), "{name}: {bald}");
        let other = cs.hair_mesh_texture(race, 0, 0, 2).expect(name);
        assert!(other.contains("Hair00_02"), "{name}: {other}");
    }
    let human = cs.hair_mesh_texture(1, 0, 0, 0).expect("human");
    assert!(human.contains("Hair03_00"), "style 1's sheet: {human}");
    assert_eq!(
        cs.hair_mesh_texture(1, 0, 1, 0),
        cs.hair_texture(1, 0, 1, 0)
    );
}

#[test]
fn a_naked_body_takes_its_face_and_underwear() {
    let Some(chain) = chain() else { return };
    let cs = CharSections::load(chain).expect("load CharSections");
    let base =
        read_mip_chain(chain, "Character\\Human\\Male\\HumanMaleSkin00_03.blp").expect("the skin");
    let comp = compose(&cs, chain, (1, 0), [3, 0, 1, 0, 0], [None; 8], None);
    assert_eq!((comp.width, comp.height), (256, 256));
    assert_eq!(comp.mips.len(), base.mips.len());
    let changed = |t: Tile| texels_differing(&rect(&base, t), &rect(&comp, t));
    assert!(changed(HEAD_LOWER) > 4000, "the lower face");
    assert!(changed(HEAD_UPPER) > 2000, "the upper face");
    assert!(changed(tile(5)) > 4000, "the pelvis");
    assert_eq!(changed(tile(3)), 0, "no male row has a torso underwear");

    let dir = "Character\\NightElf\\Female\\NightElfFemale";
    let torso = read_mip_chain(chain, &format!("{dir}NakedTorsoSkin00_00.blp")).expect("torso");
    assert_eq!((torso.width, torso.height), (128, 64));
    let comp = compose(&cs, chain, (4, 1), [0; 5], [None; 8], None);
    assert_eq!(rect(&comp, tile(3)), rect(&torso, (0, 0, 128, 64)));
}

#[test]
fn only_the_first_equipment_cells_cover_the_underwear() {
    let Some(chain) = chain() else { return };
    let cs = CharSections::load(chain).expect("load CharSections");
    let dir = "Character\\NightElf\\Female\\NightElfFemale";
    let read = |n: &str| read_mip_chain(chain, &format!("{dir}{n}00_00.blp")).expect(n);
    let (base, torso, pelvis) = (
        read("Skin"),
        read("NakedTorsoSkin"),
        read("NakedPelvisSkin"),
    );
    let sheet = |img: &MipChain| rect(img, (0, 0, 128, 64));
    let (bra, panties) = (sheet(&torso), sheet(&pelvis));
    let (bare_torso, bare_pelvis) = (rect(&base, tile(3)), rect(&base, tile(5)));
    assert_ne!(bra, bare_torso);
    assert_ne!(panties, bare_pelvis);
    let occupies = |layers: &[usize]| {
        let mut d = ItemDisplay::default();
        for &l in layers {
            d.region_textures[l] = Some("no-such-region".into());
        }
        d
    };
    let (shirt, chest, belt, pants, tabard) = (
        occupies(&[3]),
        occupies(&[3, 5]),
        occupies(&[5]),
        occupies(&[5]),
        occupies(&[3]),
    );
    let cases = [
        ("shirt", 0, &shirt, &bare_torso, &panties),
        ("tabard", 7, &tabard, &bra, &panties),
        ("pants", 3, &pants, &bra, &bare_pelvis),
        ("belt", 2, &belt, &bra, &panties),
        ("chest", 1, &chest, &bare_torso, &bare_pelvis),
    ];
    for (label, slot, display, want_torso, want_pelvis) in cases {
        let mut equipment = [None; 8];
        equipment[slot] = Some(display);
        let comp = compose(&cs, chain, (4, 1), [0; 5], equipment, None);
        let torso = texels_differing(&rect(&comp, tile(3)), want_torso);
        let pelvis = texels_differing(&rect(&comp, tile(5)), want_pelvis);
        assert_eq!((torso, pelvis), (0, 0), "{label}");
    }
    let naked = compose(&cs, chain, (4, 1), [0; 5], [None; 8], None);
    assert_eq!(rect(&naked, tile(3)), bra);
    assert_eq!(rect(&naked, tile(5)), panties);
}

#[test]
fn the_guild_emblem_paints_the_torso_of_the_guild_tabard() {
    let Some(chain) = chain() else { return };
    let cs = CharSections::load(chain).expect("load CharSections");
    let items = ItemDisplayCatalog::load(chain).expect("load ItemDisplayInfo");
    let tabard = items.get(20621).expect("the guild tabard");
    assert!(tabard.takes_guild_emblem());
    assert!(!items.get(9891).expect("a shirt").takes_guild_emblem());
    let emblem = GuildEmblem {
        emblem_style: 42,
        emblem_color: 3,
        border_style: 1,
        border_color: 7,
        background_color: 12,
    };
    let mut equipment = [None; 8];
    equipment[7] = Some(tabard);
    for step in equip_blits(&equipment, Some(emblem), false) {
        let path = step
            .candidates(0)
            .into_iter()
            .find(|p| read_mip_chain(chain, p).is_ok())
            .unwrap_or_else(|| panic!("{:?} resolves no file", step.source));
        let art = read_mip_chain(chain, &path).expect("re-read");
        let (_, _, w, h) = tile(step.layer);
        assert_eq!((art.width, art.height), (w, h), "{path}");
    }
    let plain = compose(&cs, chain, (1, 0), [3, 0, 1, 0, 0], equipment, None);
    let crested = compose(&cs, chain, (1, 0), [3, 0, 1, 0, 0], equipment, Some(emblem));
    let torso = [tile(3), tile(4)];
    let mut moved = 0;
    for (i, (a, b)) in plain.mips[0]
        .chunks(4)
        .zip(crested.mips[0].chunks(4))
        .enumerate()
    {
        if a != b {
            moved += 1;
            let (x, y) = ((i % 256) as u32, (i / 256) as u32);
            assert!(
                torso
                    .iter()
                    .any(|&(tx, ty, tw, th)| x >= tx && x < tx + tw && y >= ty && y < ty + th),
                "the emblem painted ({x}, {y}), outside the torso"
            );
        }
    }
    assert!(moved > 0);
    let other = GuildEmblem {
        background_color: 30,
        ..emblem
    };
    let recolored = compose(&cs, chain, (1, 0), [3, 0, 1, 0, 0], equipment, Some(other));
    assert_ne!(crested.mips[0], recolored.mips[0]);
    let upper_case = GuildEmblem {
        background_color: 29,
        ..emblem
    };
    for half in ["TU", "TL"] {
        let path = EmblemLayer::Background.path(&upper_case, half);
        assert!(read_mip_chain(chain, &path).is_ok(), "{path}");
    }
}

#[test]
fn a_starting_outfit_repaints_the_tiles_it_names() {
    let Some(chain) = chain() else { return };
    let cs = CharSections::load(chain).expect("load CharSections");
    let items = ItemDisplayCatalog::load(chain).expect("load ItemDisplayInfo");
    let (shirt, pants, boots) = (
        items.get(9891).expect("shirt"),
        items.get(9892).expect("pants"),
        items.get(10141).expect("boots"),
    );
    let dress = |e| compose(&cs, chain, (1, 0), [3, 0, 1, 0, 0], e, None);
    let naked = dress([None; 8]);
    let pants_only = dress([Some(shirt), None, None, Some(pants), None, None, None, None]);
    let dressed = dress([
        Some(shirt),
        None,
        None,
        Some(pants),
        Some(boots),
        None,
        None,
        None,
    ]);
    let changed = |a: &MipChain, b: &MipChain, l: usize| {
        texels_differing(&rect(a, tile(l)), &rect(b, tile(l)))
    };
    assert!(changed(&naked, &dressed, 3) > 2000, "the shirt");
    assert!(changed(&naked, &dressed, 5) > 2000, "the pants");
    assert!(changed(&naked, &dressed, 7) > 1000, "the boots");
    assert_eq!(changed(&naked, &dressed, 2), 0, "the hands");
    assert!(
        changed(&pants_only, &dressed, 6) > 1000,
        "boots over the pants"
    );
}

#[test]
fn item_displays_name_their_models_left_or_right() {
    let Some(chain) = chain() else { return };
    let cat = ItemDisplayCatalog::load(chain).expect("load ItemDisplayInfo");
    let arrow = cat.get(5996).expect("an arrow");
    assert_eq!(arrow.model, [None, Some("arrowflight_01.m2".into())]);
    assert_eq!(arrow.model_texture[1].as_deref(), Some("Arrow_A_01Brown"));
    assert_eq!(
        cat.get(5998).and_then(|d| d.model[1].as_deref()),
        Some("bulletflight_01.m2")
    );
    let thrown = cat.get(16752).expect("a throwing dagger");
    assert_eq!(thrown.model[0].as_deref(), Some("thrown_1h_dagger_a_01.m2"));
    assert_eq!(
        thrown.model_texture[0].as_deref(),
        Some("Thrown_1H_Dagger_A_01Copper")
    );
    let shield = cat.get(18730).expect("a shield");
    assert_eq!(shield.model[0].as_deref(), Some("shield_round_a_01.m2"));
    assert_eq!(
        shield.model_texture[0].as_deref(),
        Some("Buckler_Damaged_A_01Purple")
    );
    assert_eq!(cat.get(15676).and_then(ItemDisplay::worn_helm_vis), None);
}

#[test]
fn creature_displays_resolve_models_scales_and_appearances() {
    let Some(chain) = chain() else { return };
    let cat = CreatureCatalog::load(chain).expect("load the creatures");
    assert!(cat.extra_len() > 1000, "{} appearances", cat.extra_len());
    let guard = cat.model(3167).expect("a Stormwind guard");
    let npc = guard.npc_appearance.expect("a character model");
    assert_eq!(
        npc.equipment,
        [14964, 7541, 7223, 0, 7224, 7225, 7255, 0, 7698, 6255]
    );
    let mut baked = 0;
    for row in raw_table(chain, "CreatureDisplayInfo", "uuuuuusssuuu") {
        let Some(m) = cat.model(row[0].parse().expect("an id")) else {
            continue;
        };
        let Some(bake) = m.npc_appearance.as_ref().and_then(|a| a.bake_name.as_ref()) else {
            continue;
        };
        assert!(m.model_path.to_ascii_lowercase().starts_with("character\\"));
        assert!(chain.contains(&format!("Textures\\BakedNpcTextures\\{bake}")));
        baked += 1;
    }
    assert!(baked > 1000, "{baked} baked NPC skins");

    let strider = cat.model(4945).expect("a sea giant");
    assert_eq!(strider.model_path, "Creature\\SeaGiant\\SeaGiant.mdx");
    let height = cat.collision_height(4945).expect("a height");
    assert!((height - 2.083).abs() < 5e-4, "{height}");
    assert!((cat.display_scale(4945).expect("a scale") - 1.75).abs() < 5e-4);
    assert!((strider.scale - 1.75).abs() < 5e-4);
    assert_eq!(cat.model_scale(4945), Some(strider.scale));
    for (display, expect) in [(49, 2.031), (56, 2.250), (60, 2.111), (1564, 1.000)] {
        let h = cat.collision_height(display).expect("a height");
        assert!((h - expect).abs() < 5e-4, "display {display}: {h}");
    }
    assert!(cat.model(0).is_none() && cat.collision_height(0).is_none());
}

#[test]
fn a_display_prints_with_the_ink_and_size_its_model_names() {
    let Some(chain) = chain() else { return };
    let cat = CreatureCatalog::load(chain).expect("load the creatures");
    let human = cat.footprint(49).expect("a human male prints");
    assert_eq!(human.texture, 1);
    assert!((human.length - 12.0 / 36.0).abs() < 1e-6);
    assert!((human.width - 10.0 / 36.0).abs() < 1e-6);
    assert_eq!(
        cat.footprint(59).map(|f| f.texture),
        Some(3),
        "a tauren's hoof"
    );
    assert!(
        cat.footprint(11686).is_none(),
        "the invisible stalker leaves none"
    );
}
