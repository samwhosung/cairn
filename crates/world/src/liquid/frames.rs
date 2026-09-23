//! A liquid's animated frames as one texture array, each frame's own mip levels laid in as the
//! file stores them.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat, TextureViewDescriptor, TextureViewDimension,
};
use blp::DecodedBlp;
use mpq::Chain;

/// Frames `1..=count` of `XTextures\<dir>\<stem>.<n>.blp`, stopping at the first that is missing,
/// not square, or another size than the first.
pub(crate) fn read_frames(chain: &Chain, dir: &str, stem: &str, count: u32) -> Vec<DecodedBlp> {
    let mut frames: Vec<DecodedBlp> = Vec::new();
    for i in 1..=count {
        let Ok(bytes) = chain.read(&format!("XTextures\\{dir}\\{stem}.{i}.blp")) else {
            break;
        };
        let Ok(blp) = blp::decode(&bytes) else {
            break;
        };
        if blp.width != blp.height || frames.first().is_some_and(|f| f.width != blp.width) {
            break;
        }
        frames.push(blp);
    }
    frames
}

/// The frames as one repeating, trilinear `2d_array`, a full chain down to 1×1. A level a frame
/// does not store repeats its nearest stored one, texel for texel. `flatten` evens out each
/// level's mean across the frames, so distant water does not pulse once a loop.
pub(crate) fn frame_array(frames: &[DecodedBlp], flatten: bool) -> Option<Image> {
    let size = frames.first()?.width;
    let levels = size.max(1).ilog2() + 1;
    let mut data = Vec::new();
    let mut spans: Vec<Vec<(usize, usize)>> = Vec::with_capacity(frames.len());
    for blp in frames {
        let stored = blp.mips.len().min(blp.mip_chain_count().max(1));
        let mut per_level = Vec::with_capacity(levels as usize);
        for level in 0..levels {
            let lw = (size >> level).max(1);
            let src = ((size / lw).max(1).trailing_zeros() as usize).min(stored - 1);
            let mip = &blp.mips[src];
            let start = data.len();
            extend_nearest(&mut data, &mip.rgba, mip.width, mip.height, lw, lw);
            per_level.push((start, data.len() - start));
        }
        spans.push(per_level);
    }
    if flatten {
        flatten_frame_dc(&mut data, &spans, levels as usize);
    }
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: frames.len() as u32,
        },
        TextureDimension::D2,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..TextureViewDescriptor::default()
    });
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::default()
    });
    Some(image)
}

/// Appends `src`, `sw × sh` RGBA8, resized to `dw × dh` by picking the texel under each output
/// texel's centre, never averaging.
fn extend_nearest(out: &mut Vec<u8>, src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) {
    if sw == dw && sh == dh {
        out.extend_from_slice(src);
        return;
    }
    if src.len() != (sw * sh * 4) as usize {
        out.extend(std::iter::repeat_n(0u8, (dw * dh * 4) as usize));
        return;
    }
    let pick = |o: u32, from: u32, to: u32| {
        let at = ((o as f32 + 0.5) * (from as f32 / to as f32)).floor() as u32;
        at.min(from - 1) as usize
    };
    for y in 0..dh {
        let sy = pick(y, sh, dh);
        for x in 0..dw {
            let at = (sy * sw as usize + pick(x, sw, dw)) * 4;
            out.extend_from_slice(&src[at..at + 4]);
        }
    }
}

/// Moves each frame's per-channel sum at every level onto the loop's mean, one byte step at a
/// time on a scattered walk, so no texel moves by more than the correction needs.
fn flatten_frame_dc(data: &mut [u8], spans: &[Vec<(usize, usize)>], levels: usize) {
    for level in 0..levels {
        let mut sums = [0i64; 4];
        let mut texels = 0usize;
        for frame in spans {
            let (start, len) = frame[level];
            for px in data[start..start + len].as_chunks::<4>().0 {
                for c in 0..4 {
                    sums[c] += i64::from(px[c]);
                }
            }
            texels += len / 4;
        }
        if texels == 0 {
            continue;
        }
        for frame in spans {
            let (start, len) = frame[level];
            let n = len / 4;
            if n == 0 {
                continue;
            }
            for c in 0..4 {
                let have: i64 = data[start..start + len]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|px| i64::from(px[c]))
                    .sum();
                let want = (sums[c] * n as i64).div_euclid(texels as i64);
                shift_channel(&mut data[start..start + len], c, want - have);
            }
        }
    }
}

fn shift_channel(level: &mut [u8], c: usize, mut delta: i64) {
    let n = level.len() / 4;
    let stride = if n.is_multiple_of(7) { 1 } else { 7 };
    let mut progress = true;
    while delta != 0 && progress {
        progress = false;
        for k in 0..n {
            if delta == 0 {
                break;
            }
            let b = &mut level[((k * stride) % n) * 4 + c];
            if delta > 0 && *b < 255 {
                *b += 1;
                delta -= 1;
                progress = true;
            } else if delta < 0 && *b > 0 {
                *b -= 1;
                delta += 1;
                progress = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_sum(data: &[u8], (start, len): (usize, usize), c: usize) -> i64 {
        data[start..start + len]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|px| i64::from(px[c]))
            .sum()
    }

    #[test]
    fn every_frame_lands_on_the_loop_mean_at_every_level() {
        let frame = |base: u8| {
            let mut level0 = Vec::new();
            for t in 0..4u8 {
                level0.extend_from_slice(&[base + t, base, base + 2 * t, 200]);
            }
            let level1 = vec![base + 1, base, base + 3, 200];
            (level0, level1)
        };
        let mut data = Vec::new();
        let mut spans = Vec::new();
        for base in [10u8, 60] {
            let (l0, l1) = frame(base);
            let s0 = (data.len(), l0.len());
            data.extend_from_slice(&l0);
            let s1 = (data.len(), l1.len());
            data.extend_from_slice(&l1);
            spans.push(vec![s0, s1]);
        }
        flatten_frame_dc(&mut data, &spans, 2);
        for (a, b) in spans[0].iter().zip(&spans[1]) {
            for c in 0..4 {
                let (a, b) = (level_sum(&data, *a, c), level_sum(&data, *b, c));
                assert!((a - b).abs() <= 1, "channel {c}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn nearest_picks_texels_and_never_blends() {
        let src: Vec<u8> = (0..16u8).flat_map(|i| [i * 10, 0, 0, 255]).collect();
        let mut out = Vec::new();
        extend_nearest(&mut out, &src, 4, 4, 2, 2);
        let reds: Vec<u8> = out.as_chunks::<4>().0.iter().map(|p| p[0]).collect();
        assert_eq!(reds, vec![50, 70, 130, 150]);
    }
}
