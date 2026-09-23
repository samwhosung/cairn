use std::collections::HashSet;
use std::io::Cursor;
use std::path::PathBuf;

use m2::{M2Format, parse_m2};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn load(chain: &Chain, name: &str) -> (Vec<u8>, M2Format) {
    let bytes = chain.read(name).unwrap_or_else(|e| panic!("{name}: {e}"));
    let format = parse_m2(&mut Cursor::new(bytes.as_slice())).unwrap_or_else(|e| panic!("{e}"));
    (bytes, format)
}

#[test]
fn every_model_parses_with_consistent_skins() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let mut models = 0;
    for entry in chain.list() {
        if !entry.name.to_ascii_lowercase().ends_with(".m2") {
            continue;
        }
        let (bytes, format) = load(&chain, &entry.name);
        let model = format.model();
        for view in 0..4 {
            let skin = model
                .parse_embedded_skin(&bytes, view)
                .unwrap_or_else(|e| panic!("{} skin {view}: {e}", entry.name));
            let name = &entry.name;
            assert!(
                skin.indices()
                    .iter()
                    .all(|&i| (i as usize) < model.vertices.len()),
                "{name}"
            );
            assert!(
                skin.triangles()
                    .iter()
                    .all(|&t| (t as usize) < skin.indices().len()),
                "{name}"
            );
            assert!(
                skin.batches()
                    .iter()
                    .all(|b| (b.skin_section_index as usize) < skin.submeshes().len()),
                "{name}"
            );
        }
        assert!(model.parse_embedded_skin(&bytes, 4).is_err());
        models += 1;
    }
    assert!(models > 9000, "{models} models");
}

#[test]
fn duplicate_attachment_ids_resolve_through_the_lookup() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (_, format) = load(
        &chain,
        "ITEM\\ObjectComponents\\WEAPON\\Stave_2H_Long_D_05.m2",
    );
    let model = format.model();
    let ids: HashSet<u16> = model.attachments.iter().map(|a| a.id).collect();
    assert_eq!(model.attachments.len(), 8);
    assert_eq!(ids.len(), 4);
    for id in 0..4 {
        let a = model.attachment(id).expect("the lookup resolves the id");
        assert_eq!(a.id, id);
    }
}

#[test]
fn negative_alpha_keys_hide_the_burnt_gate() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (_, format) = load(
        &chain,
        "World\\Kalimdor\\Tanaris\\ActiveDoodads\\TrollGate\\TanarisTrollGate.m2",
    );
    let model = format.model();
    let keys = model
        .color_alpha_tracks
        .iter()
        .chain(&model.transparency_tracks)
        .flat_map(|t| t.keys.iter().map(|&(_, v)| v));
    assert!(keys.clone().any(|v| v < -0.99), "a key at -1");
    assert!(keys.clone().any(|v| v > 0.99), "a key at +1");
}

#[test]
fn a_fly_by_carries_one_bezier_camera() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (bytes, format) = load(&chain, "Cameras\\FlyByDwarf.m2");
    let cameras = &format.model().cameras;
    assert_eq!(cameras.len(), 1);
    let camera = &cameras[0];
    assert_eq!(camera.camera_type, -1);
    assert!((camera.fov - std::f32::consts::FRAC_PI_4).abs() < 1e-4);
    assert_eq!(camera.positions.interp, 2);
    assert!(camera.positions.keys.len() > 2);
    let (start, end) = (
        camera.positions.keys[0].0,
        camera.positions.keys.last().expect("keys").0,
    );
    let mid = camera.positions.sample_ms(start + (end - start) / 2);
    assert!(mid.is_some_and(|p| p.iter().all(|c| c.is_finite())));
    assert_eq!(m2::parse_cameras(&bytes).len(), 1);
}

#[test]
fn texture_lookups_read_their_own_slots() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (_, portal) = load(
        &chain,
        "World\\Generic\\ActiveDoodads\\MagePortals\\StormwindMagePortal01.m2",
    );
    assert_eq!(portal.model().transparency_lookup, [0, 1, 2, 3]);
    assert_eq!(portal.model().texture_unit_lookup, [0]);
    let (_, waterfall) = load(
        &chain,
        "World\\Azeroth\\Elwynn\\PassiveDoodads\\Waterfall\\ElwynnTallWaterfall01.m2",
    );
    assert_eq!(waterfall.model().texture_transforms.len(), 2);
    assert_eq!(waterfall.model().texture_transform_lookup, [0, 1]);
}

#[test]
fn glass_without_uvs_is_env_mapped() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (bytes, format) = load(
        &chain,
        "World\\Generic\\Gnome\\Passive Doodads\\GnomeMachine\\GnomeSubwayGlass.m2",
    );
    let model = format.model();
    assert!(
        model
            .vertices
            .iter()
            .all(|v| v.tex_coords.x == 0.0 && v.tex_coords.y == 0.0)
    );
    let skin = model.parse_embedded_skin(&bytes, 0).expect("skin 0");
    assert!(
        skin.batches()
            .iter()
            .all(|b| model.stage_is_env_mapped(b, 0))
    );
}

#[test]
fn a_character_model_carries_its_animation_tables() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let (_, format) = load(&chain, "Character\\Human\\Male\\HumanMale.m2");
    let model = format.model();
    assert_eq!(model.playable_animation_lookup.len(), 203);
    assert!(model.owns_animation(0));
    assert!(!model.owns_animation(u16::MAX));
    let pivot = model.attachment(17).expect("attachment 17");
    assert_eq!(model.pivot_attach_z, Some(pivot.position[2]));
}
