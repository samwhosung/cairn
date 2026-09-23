use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::asset::io::AssetReaderError;
use bevy::asset::{AssetLoadError, AssetPlugin, LoadState};
use bevy::image::{CompressedImageFormats, ImageAddressMode, ImageSampler};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use blp::BlpTexels;
use mpq::Chain;
use world::Repeat;

fn data_or_skip() -> Option<PathBuf> {
    let data = std::env::var_os("WOW_DATA").map(PathBuf::from);
    if data.is_none() {
        eprintln!("skipped: WOW_DATA is not set");
    }
    data
}

#[test]
fn images_hold_what_the_decoder_decodes() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let chain = Chain::open(data).expect("open the chain");
    for (name, texels) in [
        ("Tileset\\Elwynn\\ElwynnGrassBase.blp", BlpTexels::Bc1),
        ("Interface\\AuctionFrame\\BuyoutIcon.blp", BlpTexels::Bc1),
        ("textures\\Weather\\RainDrop01.blp", BlpTexels::Bc2),
        ("Interface\\Cursor\\Buy.blp", BlpTexels::Rgba8Unorm),
        ("textures\\SunGlare.blp", BlpTexels::Rgba8Unorm),
    ] {
        let file = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
        let decoded = blp::decode(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        let native = || blp::decode_native(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(native().texels, texels, "{name}");
        let pixels: Vec<u8> = decoded.mips.iter().flat_map(|m| m.rgba.clone()).collect();
        let blocks: Vec<u8> = native().mips.into_iter().flat_map(|m| m.bytes).collect();
        for formats in [CompressedImageFormats::NONE, CompressedImageFormats::BC] {
            let image = world::blp_image(native(), formats, Repeat::BOTH);
            let desc = &image.texture_descriptor;
            assert_eq!(
                (desc.size.width, desc.size.height),
                (decoded.width, decoded.height)
            );
            assert_eq!(desc.mip_level_count as usize, decoded.mips.len(), "{name}");
            let bc = formats == CompressedImageFormats::BC;
            let (format, data) = match (bc, texels) {
                (true, BlpTexels::Bc1) => (TextureFormat::Bc1RgbaUnorm, &blocks),
                (true, BlpTexels::Bc2) => (TextureFormat::Bc2RgbaUnorm, &blocks),
                _ => (TextureFormat::Rgba8Unorm, &pixels),
            };
            assert_eq!(desc.format, format, "{name} bc={bc}");
            assert!(image.data.as_ref() == Some(data), "{name} bc={bc}");
        }
    }
}

#[test]
fn textures_load_through_the_mpq_source() {
    let Some(data) = data_or_skip() else {
        return;
    };
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    world::register_source(&mut app, &data).expect("open the chain");
    app.add_plugins(AssetPlugin::default())
        .init_asset::<Image>()
        .add_plugins(world::LoadersPlugin);
    app.finish();
    app.cleanup();

    let grass = "Tileset\\Elwynn\\ElwynnGrassBase.blp";
    let server = app.world().resource::<AssetServer>().clone();
    let clamp = Repeat { u: false, v: false };
    let repeating: Handle<Image> = server.load(world::texture_url(grass, Repeat::BOTH));
    let clamped: Handle<Image> = server.load(world::texture_url(grass, clamp));
    let missing: Handle<Image> = server.load("mpq://tileset/elwynn/nothing.blp");
    let deadline = Instant::now() + Duration::from_secs(120);
    while [&repeating, &clamped, &missing]
        .iter()
        .any(|h| matches!(server.load_state(*h), LoadState::Loading))
    {
        assert!(Instant::now() < deadline, "the loads did not finish");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }

    let chain = Chain::open(&data).expect("open the chain");
    let file = chain.read(grass).expect("read the grass");
    let expected = world::blp_image(
        blp::decode_native(&file).expect("decodes"),
        CompressedImageFormats::NONE,
        Repeat::BOTH,
    );
    let images = app.world().resource::<Assets<Image>>();
    for (handle, address) in [
        (&repeating, ImageAddressMode::Repeat),
        (&clamped, ImageAddressMode::ClampToEdge),
    ] {
        let image = images.get(handle).expect("loaded");
        assert_eq!(image.texture_descriptor, expected.texture_descriptor);
        assert!(
            image.data == expected.data,
            "the texels differ from the decoder's"
        );
        let ImageSampler::Descriptor(sampler) = &image.sampler else {
            panic!("the loader sets its own sampler");
        };
        assert_eq!(
            (sampler.address_mode_u, sampler.address_mode_v),
            (address, address)
        );
    }
    let LoadState::Failed(error) = server.load_state(&missing) else {
        panic!("a missing file fails to load");
    };
    assert!(matches!(
        *error,
        AssetLoadError::AssetReaderError(AssetReaderError::NotFound(_))
    ));
}
