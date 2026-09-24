use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::animation::AnimationClip;
use bevy::animation::graph::AnimationGraph;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use mpq::Chain;
use world::{DoodadBase, M2Model, WmoModel};

const LAMPPOST: &str = "World\\Azeroth\\Elwynn\\PassiveDoodads\\LampPost\\LampPost.mdx";
const CAMPFIRE: &str = "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.mdx";
const INN: &str = "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo";
const HUMAN_MALE: &str = "Character\\Human\\Male\\HumanMale.mdx";
const COLD_BREATH: &str = "Particles\\ColdBreath.mdl";

fn data_or_skip() -> Option<PathBuf> {
    let data = std::env::var_os("WOW_DATA").map(PathBuf::from);
    if data.is_none() {
        eprintln!("skipped: WOW_DATA is not set");
    }
    data
}

fn app(data: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let install = world::Install::open(data).expect("open the chain");
    world::register_source(&mut app, &install);
    app.add_plugins(AssetPlugin::default())
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .init_asset::<AnimationClip>()
        .init_asset::<AnimationGraph>()
        .add_plugins(world::LoadersPlugin);
    app.finish();
    app.cleanup();
    app
}

fn wait_for<A: Asset>(app: &mut App, handle: &Handle<A>) {
    let deadline = Instant::now() + Duration::from_secs(120);
    let server = app.world().resource::<AssetServer>().clone();
    while matches!(server.load_state(handle), LoadState::Loading) {
        assert!(Instant::now() < deadline, "the load did not finish");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn an_m2_loads_as_the_model_crate_reads_it() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let mut app = app(&data);
    let server = app.world().resource::<AssetServer>().clone();
    let handle: Handle<M2Model> = server.load(world::m2_url(LAMPPOST));
    let fire: Handle<M2Model> = server.load(world::m2_url(CAMPFIRE));
    wait_for(&mut app, &handle);
    wait_for(&mut app, &fire);
    let chain = Chain::open(&data).expect("open the chain");
    let expected = model::load_m2_mesh(&chain, LAMPPOST).expect("the model crate reads it");
    let bounds = model::load_m2_bounds(&chain, LAMPPOST).expect("bounds");
    let m2s = app.world().resource::<Assets<M2Model>>();
    let m = m2s.get(&handle).expect("loaded");
    assert_eq!(m.submeshes.len(), expected.len());
    for (got, want) in m.submeshes.iter().zip(&expected) {
        assert_eq!(got.geometry.positions, want.positions);
        assert_eq!(got.geometry.indices, want.indices);
        assert_eq!(got.geometry.blend, want.blend);
        assert_eq!(got.texture.is_some(), want.texture.is_some());
        assert_eq!(got.billboard.is_some(), want.billboard.is_some());
        let path = got
            .texture
            .as_ref()
            .and_then(|t| t.path())
            .map(ToString::to_string);
        let wrap = world::Repeat {
            u: want.wrap_x,
            v: want.wrap_y,
        };
        assert_eq!(
            path,
            want.texture.as_deref().map(|t| world::texture_url(t, wrap))
        );
    }
    let glow = m.submeshes.last().expect("the lamp's glow card");
    assert!(glow.geometry.additive && glow.billboard.is_some());
    let got = m.bounds.expect("header bounds");
    assert_eq!(
        (got.sphere_radius, got.bbox_min, got.bbox_max),
        (bounds.sphere_radius, bounds.bbox_min, bounds.bbox_max)
    );
    assert!(m.lights.is_empty());
    let fire = m2s.get(&fire).expect("loaded");
    assert_eq!(
        fire.lights.iter().filter(|l| l.casts()).count(),
        1,
        "its flame"
    );
}

#[test]
fn a_character_loads_its_skeleton_and_sequences() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let mut app = app(&data);
    let handle: Handle<M2Model> = app
        .world()
        .resource::<AssetServer>()
        .load(world::m2_url(HUMAN_MALE));
    wait_for(&mut app, &handle);
    let chain = Chain::open(&data).expect("open the chain");
    let bytes = chain
        .read(&HUMAN_MALE.replace(".mdx", ".m2"))
        .expect("read");
    let skeleton = model::parse_m2_skeleton(&bytes).expect("skeleton");
    let m2s = app.world().resource::<Assets<M2Model>>();
    let m = m2s.get(&handle).expect("loaded");
    assert_eq!(m.skeleton.joints.len(), skeleton.bones.len());
    assert_eq!(m.inverse_bindposes.len(), skeleton.bones.len());
    for id in [5, 6, 11, 17] {
        assert!(m.attachments.iter().any(|a| a.id == id), "attachment {id}");
    }
    let anims = m.animations.as_ref().expect("sequences");
    let stands = anims.clips.iter().filter(|c| c.anim_id == 0).count();
    assert_eq!(stands, 4, "Stand and its three variations");
    let run = anims.find_resolved(5, &|_| None).expect("a run");
    assert!(run.looping && run.move_speed > 6.9);
    assert_eq!(
        anims.find_resolved(187, &|_| None).map(|c| c.anim_id),
        Some(187)
    );
}

#[test]
fn an_idle_character_breathes_at_its_mouth_once_a_loop_and_a_puff_plays_once() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let mut app = app(&data);
    let server = app.world().resource::<AssetServer>().clone();
    let human: Handle<M2Model> = server.load(world::m2_url(HUMAN_MALE));
    let puff: Handle<M2Model> = server.load(world::m2_url(COLD_BREATH));
    wait_for(&mut app, &human);
    wait_for(&mut app, &puff);
    let m2s = app.world().resource::<Assets<M2Model>>();
    let m = m2s.get(&human).expect("loaded");
    let stand = m
        .animations
        .as_ref()
        .and_then(|a| a.find(0))
        .expect("a Stand");
    let keys: Vec<f32> = stand
        .events
        .iter()
        .filter(|e| e.ident == *b"$BTH")
        .map(|e| e.time)
        .collect();
    assert_eq!(keys.len(), 1, "one breath a loop: {keys:?}");
    assert!((keys[0] - 0.667).abs() < 0.01 && (stand.duration - 2.667).abs() < 0.01);
    assert!(
        m.attachments.iter().any(|a| a.id == 0x11),
        "the loader keeps the mouth"
    );
    let chain = Chain::open(&data).expect("open the chain");
    let bytes = chain
        .read(&HUMAN_MALE.replace(".mdx", ".m2"))
        .expect("read");
    let points = model::parse_m2_attachments(&bytes).expect("attachments");
    let at = |id: u16| {
        points
            .iter()
            .find(|a| a.id == id)
            .expect("a point")
            .position
    };
    let (mouth, head) = (at(0x11), at(11));
    assert!(
        mouth[2] > 1.5 && mouth[2] < head[2],
        "on the face, under the head"
    );
    assert!(mouth[0] > head[0], "in front of it");
    let p = m2s.get(&puff).expect("loaded");
    assert!(p.submeshes.is_empty() && p.has_emitters, "emitters alone");
    let clips = &p.animations.as_ref().expect("a sequence").clips;
    assert_eq!(clips.len(), 1);
    assert!(!clips[0].looping && (clips[0].duration - 1.5).abs() < 0.01);
}

#[test]
fn a_wmo_loads_its_groups_portals_and_doodads() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let mut app = app(&data);
    let handle: Handle<WmoModel> = app
        .world()
        .resource::<AssetServer>()
        .load(world::wmo_url(INN));
    wait_for(&mut app, &handle);
    let chain = Chain::open(&data).expect("open the chain");
    let expected = model::load_wmo(&chain, INN).expect("the model crate reads it");
    let root = model::parse_wmo_root(&chain.read(INN).expect("read")).expect("root");
    let wmos = app.world().resource::<Assets<WmoModel>>();
    let m = wmos.get(&handle).expect("loaded");
    assert_eq!(m.submeshes.len(), expected.len());
    for (got, want) in m.submeshes.iter().zip(&expected) {
        assert_eq!(got.geometry.positions, want.positions);
        assert_eq!(got.geometry.vertex_colors, want.vertex_colors);
        assert_eq!(got.geometry.wmo_batch, want.wmo_batch);
    }
    assert_eq!(m.rooms.group_nav.len(), root.group_count() as usize);
    assert_eq!(m.submesh_group.len(), m.submeshes.len());
    assert_eq!(m.rooms.portal_infos.len(), root.portals().infos.len());
    assert_eq!(m.doodads.len(), root.doodads().len());
    assert!(
        m.rooms.group_nav.iter().any(|g| g.flags & 0x8 != 0),
        "an outdoor shell"
    );
    assert!(m.rooms.group_collision_tris.iter().any(|g| !g.is_empty()));
    assert!(
        m.doodad_base
            .iter()
            .any(|b| matches!(b, DoodadBase::Interior { .. })),
        "the inn furnishes its rooms"
    );
    let props = world::prop_placements(m, 0, &Transform::IDENTITY);
    assert_eq!(
        props.len(),
        root.doodad_sets().first().map_or(0, |s| s.count as usize)
    );
}

#[test]
fn a_tile_places_its_doodads_once() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let chain = Chain::open(&data).expect("open the chain");
    let tile = terrain::load_tile_mesh(&chain, "Azeroth", 32, 48).expect("Northshire");
    let mut app = app(&data);
    let adt: Handle<world::AdtTile> = app
        .world()
        .resource::<AssetServer>()
        .load("mpq://world/maps/azeroth/azeroth_32_48.adt");
    wait_for(&mut app, &adt);
    let adts = app.world().resource::<Assets<world::AdtTile>>();
    let loaded = adts.get(&adt).expect("loaded");
    assert_eq!(loaded.doodads.len(), tile.doodads.len());
    assert_eq!(loaded.wmos.len(), tile.wmos.len());
    let mut ids: Vec<u32> = loaded.doodads.iter().map(|d| d.unique_id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        loaded.doodads.len(),
        "a tile names each doodad once"
    );
    assert!(
        loaded
            .wmos
            .iter()
            .any(|w| w.model.to_ascii_lowercase().contains("nsabbey")),
        "the abbey stands in Northshire"
    );
    assert_eq!(loaded.chunks.len(), tile.chunks.len());
    let shadowed = loaded
        .doodads
        .iter()
        .filter(|d| terrain::mcsh_shadowed_at(&loaded.chunks, d.position) == Some(true))
        .count();
    assert!(shadowed > 0 && shadowed < loaded.doodads.len());
}
