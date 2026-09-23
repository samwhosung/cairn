use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use mpq::Chain;
use world::{DoodadBase, M2Model, WmoModel};

const LAMPPOST: &str = "World\\Azeroth\\Elwynn\\PassiveDoodads\\LampPost\\LampPost.mdx";
const CAMPFIRE: &str = "World\\Azeroth\\Elwynn\\PassiveDoodads\\Campfire\\ElwynnCampfire.mdx";
const INN: &str = "World\\wmo\\Azeroth\\Buildings\\GoldshireInn\\GoldshireInn.wmo";

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
    assert_eq!(m.group_nav.len(), root.group_count() as usize);
    assert_eq!(m.submesh_group.len(), m.submeshes.len());
    assert_eq!(m.portal_infos.len(), root.portals().infos.len());
    assert_eq!(m.doodads.len(), root.doodads().len());
    assert!(
        m.group_nav.iter().any(|g| g.flags & 0x8 != 0),
        "an outdoor shell"
    );
    assert!(m.group_collision_tris.iter().any(|g| !g.is_empty()));
    assert!(
        m.doodad_base
            .iter()
            .any(|b| matches!(b, DoodadBase::Interior { .. })),
        "the inn furnishes its rooms"
    );
}
