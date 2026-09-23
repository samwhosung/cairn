use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::asset::{AssetPlugin, LoadState};
use bevy::mesh::{Indices, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use mpq::Chain;
use terrain::ChunkMesh;
use world::coords::wow_to_bevy;
use world::{AdtTile, WdtIndex};

fn data_or_skip() -> Option<PathBuf> {
    let data = std::env::var_os("WOW_DATA").map(PathBuf::from);
    if data.is_none() {
        eprintln!("skipped: WOW_DATA is not set");
    }
    data
}

fn app_without_gpu(data: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    world::register_source(&mut app, data).expect("open the chain");
    app.add_plugins(AssetPlugin::default())
        .init_asset::<Image>()
        .init_asset::<Mesh>()
        .add_plugins(world::LoadersPlugin);
    app.finish();
    app.cleanup();
    app
}

fn load<A: Asset>(app: &mut App, url: String) -> Handle<A> {
    let handle: Handle<A> = app.world().resource::<AssetServer>().load(url);
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let state = app.world().resource::<AssetServer>().load_state(&handle);
        match state {
            LoadState::Loaded => return handle,
            LoadState::Failed(e) => panic!("{e}"),
            _ => assert!(Instant::now() < deadline, "the load did not finish"),
        }
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn tiles_load_as_the_terrain_crate_meshes_them() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let chain = Chain::open(&data).expect("open the chain");
    let mut app = app_without_gpu(&data);
    for (x, y) in [(32, 48), (38, 27)] {
        let url = format!("mpq://world/maps/azeroth/azeroth_{x}_{y}.adt");
        let handle: Handle<AdtTile> = load(&mut app, url);
        let meshed = terrain::load_tile_mesh(&chain, "Azeroth", x, y).expect("the tile meshes");
        let drawn: Vec<&ChunkMesh> = meshed
            .chunks
            .iter()
            .filter(|c| c.indices.len() >= 3)
            .collect();
        let world = app.world();
        let tile = world
            .resource::<Assets<AdtTile>>()
            .get(&handle)
            .expect("loaded");
        let (mesh, _) = tile.mesh.as_ref().expect("the tile draws");
        let mesh = world
            .resource::<Assets<Mesh>>()
            .get(mesh)
            .expect("its mesh");
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("the mesh has positions");
        };
        let expected: Vec<[f32; 3]> = drawn
            .iter()
            .flat_map(|c| c.positions.iter().map(|p| wow_to_bevy(*p).to_array()))
            .collect();
        assert!(*positions == expected, "{x}_{y}: positions");
        let indices: usize = drawn.iter().map(|c| c.indices.len()).sum();
        assert_eq!(mesh.indices().map(Indices::len), Some(indices), "{x}_{y}");

        let images = world.resource::<Assets<Image>>();
        let alpha = images.get(&tile.alpha_array).expect("the alpha maps");
        let maps: Vec<u8> = drawn
            .iter()
            .filter_map(|c| c.alpha_map.clone())
            .flatten()
            .collect();
        assert!(
            alpha.data.as_deref() == Some(maps.as_slice()),
            "{x}_{y}: alpha maps"
        );
        let shadow = images.get(&tile.shadow_array).expect("the shadow maps");
        let maps: Vec<u8> = drawn
            .iter()
            .filter_map(|c| c.shadow.clone())
            .flatten()
            .collect();
        assert!(
            shadow.data.as_deref() == Some(maps.as_slice()),
            "{x}_{y}: shadows"
        );

        let layers = images.get(&tile.layer_array).expect("the ground textures");
        let names: HashSet<String> = drawn
            .iter()
            .flat_map(|c| c.layer_textures.iter().take(4))
            .map(|n| n.to_ascii_lowercase())
            .collect();
        let desc = &layers.texture_descriptor;
        assert_eq!(desc.format, TextureFormat::Rgba8Unorm);
        assert_eq!(desc.mip_level_count, 8);
        assert_eq!(desc.size.depth_or_array_layers as usize, names.len() + 1);
    }
}

#[test]
fn the_tile_index_is_the_wdt_crates() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let chain = Chain::open(&data).expect("open the chain");
    let mut app = app_without_gpu(&data);
    for map in ["Azeroth", "Kalimdor"] {
        let dir = map.to_ascii_lowercase();
        let handle: Handle<WdtIndex> = load(&mut app, format!("mpq://world/maps/{dir}/{dir}.wdt"));
        let bytes = chain
            .read(&format!("World\\Maps\\{map}\\{map}.wdt"))
            .expect("the WDT");
        let wdt = wdt::WdtReader::new(Cursor::new(bytes))
            .read()
            .expect("parses");
        let index = app
            .world()
            .resource::<Assets<WdtIndex>>()
            .get(&handle)
            .expect("loaded");
        for (x, y) in (0..64).flat_map(|x| (0..64).map(move |y| (x, y))) {
            let has = wdt
                .get_tile(x as usize, y as usize)
                .is_some_and(|t| t.has_adt);
            assert_eq!(index.has_tile(x, y), has, "{map} {x}_{y}");
        }
    }
}
