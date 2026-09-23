use std::path::PathBuf;

use model::{
    BillboardKind, CharSkinSlot, CoverageReader, DEGENERATE_RING_FOOTPRINT, ModelBlend,
    SHIPPED_GLUE_SCENES, authored_half_height, glue_art_extent, load_m2_bone_spins, load_m2_bounds,
    load_m2_mesh, load_wmo, load_wmo_collision_tris, non_separable_billboard_bones,
    parse_m2_animations, parse_m2_camera, parse_m2_global_sequence_bones, parse_m2_lights,
    parse_m2_render_submeshes, parse_wmo_root, wmo_group_fixed_colors, wmo_group_header,
    wmo_group_raw_colors,
};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn read(chain: &Chain, name: &str) -> Vec<u8> {
    chain.read(name).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn tauren_fur_binds_the_extra_skin_slot() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let subs = load_m2_mesh(&chain, "Character\\Tauren\\Male\\TaurenMale.m2").expect("load");
    let fur: Vec<_> = subs
        .iter()
        .filter(|s| s.char_slot == Some(CharSkinSlot::SkinExtra))
        .collect();
    assert!(fur.len() > 20, "{} fur batches", fur.len());
    assert!(fur.iter().all(|s| s.texture.is_none()));
    assert!(
        fur.iter()
            .any(|s| s.blend == ModelBlend::Opaque && !s.two_sided)
    );
    assert!(
        fur.iter()
            .any(|s| s.blend == ModelBlend::AlphaTest && s.two_sided)
    );
}

#[test]
fn a_weapon_reflection_layer_multiplies() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let subs = load_m2_mesh(
        &chain,
        "Item\\ObjectComponents\\Weapon\\Axe_2H_Horde_C_01.m2",
    )
    .expect("load");
    assert_eq!(subs.len(), 3);
    let reflect = subs
        .iter()
        .find(|s| {
            s.texture
                .as_deref()
                .is_some_and(|t| t.to_ascii_uppercase().contains("ARMORREFLECT"))
        })
        .expect("the reflection layer");
    assert_eq!(reflect.blend, ModelBlend::Mod2x);
    assert!(!reflect.additive && reflect.no_depth_write);
}

#[test]
fn billboard_cards_split_only_when_rigidly_separable() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let welded = "Item\\ObjectComponents\\Shoulder\\LShoulder_Plate_PVPAlliance_A_01.m2";
    let subs = load_m2_mesh(&chain, welded).expect("load");
    assert_eq!(subs.len(), 1);
    assert!(subs[0].billboard.is_none() && subs[0].welded_billboard);
    assert_eq!(subs[0].positions.len(), 152);
    assert_eq!(non_separable_billboard_bones(&read(&chain, welded)), [1, 2]);

    let detached = "Item\\ObjectComponents\\Weapon\\Sword_2H_PVPAlliance_A_01.m2";
    let subs = load_m2_mesh(&chain, detached).expect("load");
    let cards: Vec<_> = subs.iter().filter(|s| s.billboard.is_some()).collect();
    assert_eq!(cards.len(), 2);
    assert!(cards.iter().all(|c| c.positions.len() == 4));
    assert!(non_separable_billboard_bones(&read(&chain, detached)).is_empty());
}

#[test]
fn the_quest_marker_bobs_low_and_raised() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let subs = load_m2_mesh(&chain, "Interface\\Buttons\\TalkToMeQuestionMark.mdx").expect("load");
    let card = subs
        .iter()
        .find_map(|s| s.billboard.as_ref())
        .expect("a billboard");
    assert_eq!(card.kind, BillboardKind::LockZ);
    let bob = |id: u16| {
        card.seq_translations
            .iter()
            .find(|(a, _)| *a == id)
            .map_or_else(|| panic!("anim {id} bakes"), |(_, l)| l)
    };
    assert_eq!(bob(0).duration_ms, 1533);
    assert!(bob(0).keys.iter().all(|(_, v)| v[2] <= 0.0));
    assert!(bob(190).keys.iter().all(|(_, v)| v[2] > 0.4));
}

#[test]
fn the_human_eyelid_blinks_on_a_global_sequence() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let gs = parse_m2_global_sequence_bones(&read(&chain, "Character\\Human\\Male\\HumanMale.m2"));
    assert_eq!(gs.len(), 1);
    assert_eq!(gs[0].bone, 75);
    let scale = gs[0].scale.as_ref().expect("a scale channel");
    assert!(scale.period_ms > 1000);
    assert!(scale.keys[0].1.iter().all(|c| c.abs() < 1e-3));
    assert!(
        scale
            .keys
            .iter()
            .any(|(_, v)| v.iter().all(|c| (c - 1.0).abs() < 1e-3))
    );
}

#[test]
fn a_band_with_no_keys_holds_its_window_key() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let seqs = parse_m2_animations(&read(&chain, "Creature\\Zombie\\Zombie.m2"));
    for anim in [4, 5] {
        let seq = seqs.iter().find(|s| s.anim_id == anim).expect("sequence");
        let root = seq.bones.iter().find(|b| b.bone == 0).expect("root bone");
        assert_eq!(root.translation, [(0.0, [0.0, 0.0, 0.0])]);
    }
}

#[test]
fn the_voidwalker_hides_its_death_armour_while_standing() {
    const STAND: usize = 0;
    const DEATH: usize = 10;
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = read(&chain, "Creature\\VoidWalker\\VoidWalker.m2");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    for batch in [0, 1] {
        let alpha = subs[batch].alpha_anim.as_ref().expect("alpha tracks");
        assert!(alpha.sample(Some(STAND), 1.0, 0.0) <= 0.0);
        let peak = (0..32u16)
            .map(|s| alpha.sample(Some(DEATH), 3.0 * f32::from(s) / 32.0, 0.0))
            .fold(0.0f32, f32::max);
        assert!(peak > 0.9, "batch {batch} peaks at {peak}");
    }
}

#[test]
fn a_waterfall_scrolls_two_layers_at_different_rates() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = read(
        &chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
    );
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    let loops: Vec<_> = subs.iter().filter_map(|s| s.uv_anim.as_ref()).collect();
    assert_eq!(loops.len(), 2);
    let end = |i: usize| loops[i].keys.last().expect("keys").1[1];
    assert!((end(0) - end(1)).abs() > 0.5);
}

#[test]
fn the_shipped_glue_table_matches_the_measurement() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for scene in SHIPPED_GLUE_SCENES {
        let token = scene.token;
        let bytes = read(
            &chain,
            &format!("Interface\\Glues\\Models\\UI_{token}\\UI_{token}.m2"),
        );
        let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
        let cam = parse_m2_camera(&bytes, 0).expect("camera 0");
        let mut reader = CoverageReader::new(&chain);
        let ext = glue_art_extent(&subs, &cam, |s| reader.coverage(s).expect("texture"));
        assert!(
            (ext.half_w - scene.art.half_w).abs() < 2e-3
                && (ext.half_h - scene.art.half_h).abs() < 2e-3,
            "UI_{token}: measured {ext:?}, table {:?}",
            scene.art
        );
        assert!((cam.fov - scene.fov).abs() < 1e-6, "UI_{token}");
        assert!(ext.half_w / authored_half_height(cam.fov) < 16.0 / 9.0);
    }
}

#[test]
fn stormwind_loads_whole() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let name = "World\\wmo\\Azeroth\\Buildings\\Stormwind\\Stormwind.wmo";
    let root = parse_wmo_root(&read(&chain, name)).expect("root");
    assert_eq!(root.group_count(), 306);
    assert!(!root.portals().refs.is_empty());
    let subs = load_wmo(&chain, name).expect("load");
    assert!(
        subs.iter()
            .all(|s| s.wmo_batch.is_some() && s.billboard.is_none())
    );
    assert!(subs.iter().any(|s| s.interior) && subs.iter().any(|s| !s.interior));
    let collision = load_wmo_collision_tris(&chain, name).expect("collision");
    assert_eq!(collision.indices.len() % 3, 0);
    assert!(collision.triangle_count() > 10_000);
}

#[test]
fn selection_rings_match_the_client() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for (path, radius) in [
        ("Creature\\Chicken\\Chicken.mdx", 0.572_f32),
        ("Character\\Human\\Female\\HumanFemale.mdx", 0.731),
        ("Character\\Human\\Male\\HumanMale.mdx", 0.841),
        ("Creature\\Horse\\Horse.mdx", 1.295),
    ] {
        let b = load_m2_bounds(&chain, path).expect("bounds");
        assert!(
            (b.ring_footprint - radius).abs() < 0.01,
            "{path}: {}",
            b.ring_footprint
        );
    }
    let stalker =
        load_m2_bounds(&chain, "Creature\\InvisibleStalker\\InvisibleStalker.mdx").expect("bounds");
    assert_eq!((stalker.bbox_min, stalker.bbox_max), ([0.0; 3], [0.0; 3]));
    assert!((stalker.ring_footprint - DEGENERATE_RING_FOOTPRINT).abs() < f32::EPSILON);
}

#[test]
fn a_swimming_body_drops_its_camera_pivot() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for (path, drop) in [
        ("Character\\Human\\Male\\HumanMale.mdx", 0.388_257_1_f32),
        ("Character\\Tauren\\Male\\TaurenMale.mdx", 0.423_612_4),
        ("Creature\\Chicken\\Chicken.mdx", 0.0),
    ] {
        let b = load_m2_bounds(&chain, path).expect("bounds");
        assert!(
            (b.swim_pivot_drop - drop).abs() < 1e-5,
            "{path}: {}",
            b.swim_pivot_drop
        );
    }
}

#[test]
fn spell_ground_rings_are_ground_quads() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = read(&chain, "Spells\\BattleShout_Cast_Base.m2");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    let mut bones: Vec<u16> = subs
        .iter()
        .map(|s| s.ground_quad().expect("a crescent").bone)
        .collect();
    bones.sort_unstable();
    assert_eq!(bones, [1, 2, 3, 5, 6, 7]);

    let bytes = read(&chain, "SPELLS\\Flare_State_Base.m2");
    let subs = parse_m2_render_submeshes(&bytes, "", &[]).expect("parse");
    let quads: Vec<_> = subs
        .iter()
        .filter_map(model::RenderSubmesh::ground_quad)
        .collect();
    assert_eq!(quads.len(), 2);
    for q in quads {
        assert!((q.tint[0] - 0.992).abs() < 1e-3 && (q.tint[1] - 0.467).abs() < 1e-3);
        assert!((q.corners[3][0] - q.corners[0][0] - 13.89).abs() < 0.02);
    }
}

#[test]
fn a_sky_spins_its_three_belts() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let spins =
        load_m2_bone_spins(&chain, "Environments\\Stars\\CavernsOfTimeSky.m2").expect("spins");
    let mut bones: Vec<u16> = spins.keys().copied().collect();
    bones.sort_unstable();
    assert_eq!(bones, [1, 2, 3]);
    assert!(
        spins
            .values()
            .all(|s| (s.duration - 66.667).abs() < 0.01 && s.interp)
    );
}

#[test]
fn a_lamp_hums_through_one_looping_event() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = read(
        &chain,
        "World\\Generic\\NightElf\\Passive Doodads\\Lamps\\KalidarStreetLamp01.m2",
    );
    let seqs = parse_m2_animations(&bytes);
    assert_eq!(seqs.len(), 1);
    let stand = &seqs[0];
    assert!(stand.anim_id == 0 && stand.looping && stand.is_rest_pose());
    assert_eq!(stand.events.len(), 1);
    assert_eq!(&stand.events[0].ident, b"$DSL");
    assert_eq!(stand.events[0].data, 3378);
}

#[test]
fn a_campfire_and_a_torch_cast_warm_point_lights() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let campfire = parse_m2_lights(&read(
        &chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.m2",
    ));
    assert_eq!(campfire.len(), 1);
    let d = campfire[0].diffuse_color;
    assert!(campfire[0].is_point() && (d[0] - 0.71).abs() < 0.03 && d[2] < 0.05);

    let torch = parse_m2_lights(&read(
        &chain,
        "Item\\ObjectComponents\\Weapon\\Club_1H_Torch_A_01.m2",
    ));
    assert_eq!(torch.len(), 1);
    assert!(torch[0].casts() && torch[0].bone == 9);
    assert!((torch[0].position[0] - 0.5765).abs() < 0.01);
}

#[test]
fn a_group_cut_one_byte_short_keeps_its_header_and_bake() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let bytes = read(&chain, "World\\wmo\\Lorderon\\Undercity\\Undercity_144.wmo");
    let declared = u32::from_le_bytes([bytes[0x10], bytes[0x11], bytes[0x12], bytes[0x13]]);
    assert_eq!(0x14 + declared as usize, bytes.len() + 1);
    let h = wmo_group_header(&bytes).expect("header");
    assert_eq!(
        (h.flags, h.portal_ref_start, h.portal_ref_count),
        (0xa805, 397, 2)
    );
    let colors = wmo_group_raw_colors(&bytes).expect("a vertex colour per vertex");
    assert_eq!(colors.len(), 290);
}

#[test]
fn doorways_to_the_outside_whiten_their_vertex_colours() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let stem = "World\\wmo\\Azeroth\\Buildings\\NSabbey\\NSabbey";
    let root = parse_wmo_root(&read(&chain, &format!("{stem}.wmo"))).expect("root");
    let changed = |gi: u32| {
        let g = read(&chain, &format!("{stem}_{gi:03}.wmo"));
        let raw = wmo_group_raw_colors(&g).expect("colours");
        let fixed = wmo_group_fixed_colors(&g, &root).expect("colours");
        (
            raw.iter().zip(&fixed).filter(|(a, b)| a != b).count(),
            raw.len(),
        )
    };
    assert_eq!(changed(1), (0, 678));
    assert_eq!(changed(3), (10, 506));
}

#[test]
fn every_wmo_in_the_install_parses() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let mut checked = 0;
    for entry in chain.list() {
        if !entry.name.to_ascii_lowercase().ends_with(".wmo") {
            continue;
        }
        let bytes = read(&chain, &entry.name);
        if bytes.is_empty() {
            continue;
        }
        let ok = if bytes.len() > 16 && &bytes[12..16] == b"PGOM" {
            wmo_group_header(&bytes).is_some()
        } else {
            parse_wmo_root(&bytes).is_ok()
        };
        assert!(ok, "{}", entry.name);
        checked += 1;
    }
    assert!(checked > 5000, "{checked}");
}
