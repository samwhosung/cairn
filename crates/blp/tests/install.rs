use std::path::PathBuf;

use blp::{BlpTexels, decode, decode_level, decode_native};
use mpq::Chain;

fn chain_or_skip() -> Option<Chain> {
    let Some(data) = std::env::var_os("WOW_DATA").map(PathBuf::from) else {
        eprintln!("skipped: WOW_DATA is not set");
        return None;
    };
    Some(Chain::open(data).expect("open the chain"))
}

fn level_stored_size(file: &[u8], level: usize) -> usize {
    u32::from_le_bytes(
        file[84 + level * 4..88 + level * 4]
            .try_into()
            .expect("4 bytes"),
    ) as usize
}

#[test]
fn every_kind_decodes_both_ways_alike() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    for (name, texels, size, levels) in [
        ("Interface\\Cursor\\Buy.blp", BlpTexels::Rgba8Unorm, 32, 5),
        ("Interface\\Buttons\\BLUEGRAD64.blp", BlpTexels::Bc1, 64, 6),
        ("Interface\\Cooldown\\cooldown.blp", BlpTexels::Bc2, 32, 5),
        ("textures\\SunGlare.blp", BlpTexels::Rgba8Unorm, 0, 1),
    ] {
        let file = chain.read(name).unwrap_or_else(|e| panic!("{e}"));
        let decoded = decode(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        let native = decode_native(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(native.texels, texels, "{name}");
        if size > 0 {
            assert_eq!((decoded.width, decoded.height), (size, size), "{name}");
        }
        assert_eq!(decoded.mips.len(), levels, "{name}");
        assert_eq!(native.mips.len(), levels, "{name}");
        for (level, (d, n)) in decoded.mips.iter().zip(&native.mips).enumerate() {
            assert_eq!((d.width, d.height), (n.width, n.height), "{name} {level}");
            assert_eq!(d.rgba.len(), d.width as usize * d.height as usize * 4);
            assert_eq!(n.bytes.len(), texels.level_bytes(n.width, n.height));
            assert_eq!(
                decode_level(texels, n.width, n.height, &n.bytes),
                d.rgba,
                "{name} {level}"
            );
        }
    }
}

#[test]
fn an_unknown_alpha_type_without_alpha_bits_is_opaque() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let file = chain
        .read("Particles\\LeafBrown.blp")
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!((file[8], file[9], file[10]), (1, 0, 2));
    let decoded = decode(&file).expect("decodes");
    assert!(
        decoded.mips[0]
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|px| px[3] == 255)
    );
}

#[test]
fn raindrop_levels_narrower_than_a_block_are_whole() {
    let Some(chain) = chain_or_skip() else {
        return;
    };
    let file = chain
        .read("textures\\Weather\\RainDrop01.blp")
        .unwrap_or_else(|e| panic!("{e}"));
    let native = decode_native(&file).expect("decodes");
    assert_eq!(native.texels, BlpTexels::Bc2);
    assert_eq!((native.width, native.height), (16, 128));
    let short: Vec<_> = native
        .mips
        .iter()
        .enumerate()
        .filter(|(level, mip)| level_stored_size(&file, *level) < mip.bytes.len())
        .map(|(level, _)| level)
        .collect();
    assert!(!short.is_empty(), "some level is stored short of its grid");
    for level in short {
        let bytes = &native.mips[level].bytes;
        let stored = level_stored_size(&file, level);
        assert!(bytes[stored..].iter().any(|&x| x != 0), "level {level}");
    }
}
