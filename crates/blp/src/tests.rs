use crate::header::{HEADER_SIZE, MAX_DIM, PALETTE_SIZE};
use crate::{BlpTexels, Error, decode, decode_level, decode_native};

const PIXELS_AT: u32 = (HEADER_SIZE + PALETTE_SIZE) as u32;

#[derive(Clone, Copy, Default)]
struct Spec {
    compression: u8,
    alpha_bits: u8,
    alpha_type: u8,
    has_mipmaps: bool,
    width: u32,
    height: u32,
    offsets: [u32; 16],
    sizes: [u32; 16],
}

impl Spec {
    fn header(&self) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_SIZE];
        b[0..4].copy_from_slice(b"BLP2");
        b[4..8].copy_from_slice(&1u32.to_le_bytes());
        b[8] = self.compression;
        b[9] = self.alpha_bits;
        b[10] = self.alpha_type;
        b[11] = u8::from(self.has_mipmaps);
        b[12..16].copy_from_slice(&self.width.to_le_bytes());
        b[16..20].copy_from_slice(&self.height.to_le_bytes());
        for i in 0..16 {
            b[20 + i * 4..24 + i * 4].copy_from_slice(&self.offsets[i].to_le_bytes());
            b[84 + i * 4..88 + i * 4].copy_from_slice(&self.sizes[i].to_le_bytes());
        }
        b
    }

    fn file(&self, payload: &[u8]) -> Vec<u8> {
        let mut b = self.header();
        b.resize(HEADER_SIZE + PALETTE_SIZE, 0);
        b.extend_from_slice(payload);
        b
    }
}

fn level_offset(blp: &[u8], level: usize) -> usize {
    u32::from_le_bytes(
        blp[20 + level * 4..24 + level * 4]
            .try_into()
            .expect("4 bytes"),
    ) as usize
}

fn mipmapped_8x8_dxt(alpha_type: u8, block_bytes: usize) -> Vec<u8> {
    let mut spec = Spec {
        compression: 2,
        alpha_bits: 8,
        alpha_type,
        has_mipmaps: true,
        width: 8,
        height: 8,
        ..Spec::default()
    };
    let mut payload = Vec::new();
    for (i, (w, h)) in [(8u32, 8u32), (4, 4), (2, 2), (1, 1)]
        .into_iter()
        .enumerate()
    {
        let n = w.div_ceil(4) as usize * h.div_ceil(4) as usize * block_bytes;
        spec.offsets[i] = PIXELS_AT + payload.len() as u32;
        spec.sizes[i] = n as u32;
        payload.extend((0..n).map(|k| (k as u32 * 37 + i as u32 * 11) as u8));
    }
    spec.file(&payload)
}

#[test]
fn huge_dimensions_are_rejected_before_anything_is_read() {
    let spec = Spec {
        compression: 3,
        width: 65535,
        height: 65535,
        ..Spec::default()
    };
    assert!(matches!(
        decode(&spec.header()),
        Err(Error::DimensionsTooLarge {
            width: 65535,
            height: 65535
        })
    ));
    for (width, height) in [(MAX_DIM + 1, 4), (4, MAX_DIM + 1)] {
        let spec = Spec {
            width,
            height,
            ..spec
        };
        assert!(matches!(
            decode(&spec.header()),
            Err(Error::DimensionsTooLarge { .. })
        ));
    }
}

#[test]
fn short_or_foreign_input_is_not_blp2() {
    let mut b = vec![0u8; 10];
    b[0..4].copy_from_slice(b"BLP2");
    assert!(matches!(decode(&b), Err(Error::NotBlp2)));
    assert!(matches!(decode(&[]), Err(Error::NotBlp2)));
    assert!(matches!(decode(b"not a blp at all"), Err(Error::NotBlp2)));
}

#[test]
fn native_blocks_are_the_files_own_and_decode_to_the_same_pixels() {
    for (alpha_type, texels, block_bytes) in [
        (0u8, BlpTexels::Bc1, 8usize),
        (1, BlpTexels::Bc2, 16),
        (7, BlpTexels::Bc3, 16),
        (2, BlpTexels::Bc1, 8),
    ] {
        let b = mipmapped_8x8_dxt(alpha_type, block_bytes);
        let native = decode_native(&b).expect("decodes natively");
        let decoded = decode(&b).expect("decodes to pixels");

        assert_eq!(native.texels, texels, "alpha_type {alpha_type}");
        assert!(native.texels.is_block_compressed());
        assert_eq!(native.mip_chain_count(), 3);
        assert_eq!(decoded.mip_chain_count(), 3);
        assert_eq!(native.mips.len(), 3);
        assert_eq!(decoded.mips.len(), 3);

        for (level, (n, d)) in native.mips.iter().zip(&decoded.mips).enumerate() {
            assert_eq!((n.width, n.height), (d.width, d.height));
            assert_eq!(n.bytes.len(), texels.level_bytes(n.width, n.height));
            let off = level_offset(&b, level);
            assert_eq!(&n.bytes[..], &b[off..off + n.bytes.len()], "level {level}");
            assert_eq!(
                decode_level(texels, n.width, n.height, &n.bytes),
                d.rgba,
                "level {level}"
            );
        }
    }
}

#[test]
fn native_decodes_the_kinds_with_no_block_form() {
    let mut spec = Spec {
        compression: 3,
        alpha_bits: 8,
        width: 2,
        height: 2,
        ..Spec::default()
    };
    spec.offsets[0] = PIXELS_AT;
    spec.sizes[0] = 16;
    let b = spec.file(&[
        10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
    ]);
    let native = decode_native(&b).expect("decodes natively");
    assert_eq!(native.texels, BlpTexels::Rgba8Unorm);
    assert!(!native.texels.is_block_compressed());
    assert_eq!(
        native.mips[0].bytes,
        decode(&b).expect("decodes").mips[0].rgba
    );
    assert_eq!(native.mips[0].bytes.len(), 16);
}

#[test]
fn a_level_stored_short_is_completed_from_the_bytes_after_it() {
    let mut b = mipmapped_8x8_dxt(0, 8);
    let short = 3u32;
    b[84 + 2 * 4..88 + 2 * 4].copy_from_slice(&short.to_le_bytes());
    let off = level_offset(&b, 2);
    let native = decode_native(&b).expect("decodes");
    let tail = native.mips.last().expect("levels");
    assert_eq!((tail.width, tail.height), (2, 2));
    assert_eq!(&tail.bytes[..], &b[off..off + 8]);
    assert!(tail.bytes[short as usize..].iter().any(|&x| x != 0));
    let decoded = decode(&b).expect("decodes");
    assert_eq!(
        decode_level(BlpTexels::Bc1, 2, 2, &tail.bytes),
        decoded.mips.last().expect("levels").rgba
    );
}

fn dxt3_4x16() -> Spec {
    Spec {
        compression: 2,
        alpha_bits: 8,
        alpha_type: 1,
        has_mipmaps: true,
        width: 4,
        height: 16,
        ..Spec::default()
    }
}

#[test]
fn a_level_narrower_than_a_block_spans_into_the_next() {
    let mut spec = dxt3_4x16();
    let mut payload = Vec::new();
    for (i, (blocks, fill)) in [(4usize, 0x10u8), (1, 0x20), (1, 0x30), (1, 0x40)]
        .into_iter()
        .enumerate()
    {
        spec.offsets[i] = PIXELS_AT + payload.len() as u32;
        spec.sizes[i] = (blocks * 16) as u32;
        payload.extend(std::iter::repeat_n(fill, blocks * 16));
    }
    let native = decode_native(&spec.file(&payload)).expect("decodes");
    assert_eq!(native.texels, BlpTexels::Bc2);
    assert_eq!(native.mips.len(), 4);
    let two_by_eight = &native.mips[1];
    assert_eq!((two_by_eight.width, two_by_eight.height), (2, 8));
    assert_eq!(two_by_eight.bytes.len(), 32);
    assert!(two_by_eight.bytes[..16].iter().all(|&x| x == 0x20));
    assert!(two_by_eight.bytes[16..].iter().all(|&x| x == 0x30));
    assert!(native.mips[2].bytes.iter().all(|&x| x == 0x30));
    assert!(native.mips[3].bytes.iter().all(|&x| x == 0x40));
}

#[test]
fn a_narrow_level_the_file_ends_inside_is_zero_padded() {
    let mut spec = dxt3_4x16();
    spec.offsets[0] = PIXELS_AT;
    spec.sizes[0] = 64;
    spec.offsets[1] = PIXELS_AT + 64;
    spec.sizes[1] = 16;
    let mut payload = vec![0x10u8; 64];
    payload.extend([0x20u8; 16]);
    let native = decode_native(&spec.file(&payload)).expect("decodes");
    assert_eq!(native.mips.len(), 2);
    let two_by_eight = &native.mips[1];
    assert_eq!(two_by_eight.bytes.len(), 32);
    assert!(two_by_eight.bytes[..16].iter().all(|&x| x == 0x20));
    assert!(two_by_eight.bytes[16..].iter().all(|&x| x == 0));
}

#[test]
fn uncompressed_is_bgra_and_reads_level_0_without_mipmaps() {
    let mut spec = Spec {
        compression: 3,
        alpha_bits: 8,
        width: 2,
        height: 2,
        ..Spec::default()
    };
    spec.offsets[0] = PIXELS_AT;
    spec.sizes[0] = 16;
    let b = spec.file(&[
        10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160,
    ]);
    let decoded = decode(&b).expect("decodes");
    assert_eq!((decoded.width, decoded.height), (2, 2));
    assert_eq!(decoded.mip_chain_count(), 0);
    assert_eq!(decoded.mips.len(), 1);
    let mip = &decoded.mips[0];
    assert_eq!((mip.width, mip.height), (2, 2));
    assert_eq!(
        mip.rgba,
        [
            30, 20, 10, 40, 70, 60, 50, 80, 110, 100, 90, 120, 150, 140, 130, 160
        ]
    );
}

#[test]
fn palettized_is_bgra_with_packed_alpha() {
    let mut spec = Spec {
        compression: 1,
        alpha_bits: 4,
        width: 2,
        height: 1,
        ..Spec::default()
    };
    spec.offsets[0] = PIXELS_AT;
    spec.sizes[0] = 3;
    let mut b = spec.header();
    let mut palette = vec![0u8; PALETTE_SIZE];
    palette[4..8].copy_from_slice(&[1, 2, 3, 4]);
    palette[8..12].copy_from_slice(&[5, 6, 7, 8]);
    b.extend(palette);
    b.extend([1, 2, 0xA5]);
    let decoded = decode(&b).expect("decodes");
    assert_eq!(decoded.mips[0].rgba, [3, 2, 1, 0x55, 7, 6, 5, 0xAA]);
}

#[test]
fn a_zero_width_dxt_texture_decodes_to_no_pixels() {
    let mut spec = Spec {
        compression: 2,
        width: 0,
        height: 4,
        ..Spec::default()
    };
    spec.offsets[0] = PIXELS_AT;
    spec.sizes[0] = 8;
    let decoded = decode(&spec.file(&[0; 8])).expect("decodes");
    assert!(decoded.mips[0].rgba.is_empty());
    assert!(decode_level(BlpTexels::Bc1, 0, 4, &[0; 8]).is_empty());
}
