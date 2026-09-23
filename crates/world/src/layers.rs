use bevy::asset::RenderAssetUsages;
use bevy::image::{
    CompressedImageFormats, Image, ImageAddressMode, ImageFilterMode, ImageSampler,
    ImageSamplerDescriptor,
};
use bevy::render::render_resource::{
    Extent3d, TextureDataOrder, TextureDimension, TextureFormat, TextureViewDescriptor,
    TextureViewDimension,
};
use blp::{BlpTexels, NativeBlp};

pub(crate) const LAYER_SIZE: u32 = 256;
/// Levels per ground texture: 256 down to 2, as the client uploads them.
pub(crate) const LAYER_MIPS: u32 = 8;
/// The layer array's first layer, where a ground texture that failed to load points.
pub(crate) const FALLBACK_LAYER: u32 = 0;
const MISSING_TEXTURE_RGBA: [u8; 4] = [107, 133, 82, 0];

pub(crate) struct Levels {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) texels: BlpTexels,
    pub(crate) mips: Vec<Vec<u8>>,
}

impl From<NativeBlp> for Levels {
    fn from(blp: NativeBlp) -> Self {
        Self {
            width: blp.width,
            height: blp.height,
            texels: blp.texels,
            mips: blp.mips.into_iter().map(|m| m.bytes).collect(),
        }
    }
}

impl Levels {
    fn size(&self, level: usize) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }

    fn into_rgba8(self) -> Self {
        if !self.texels.is_block_compressed() {
            return self;
        }
        let mips = (0..self.mips.len())
            .map(|i| {
                let (w, h) = self.size(i);
                blp::decode_level(self.texels, w, h, &self.mips[i])
            })
            .collect();
        Self {
            texels: BlpTexels::Rgba8Unorm,
            mips,
            ..self
        }
    }
}

pub(crate) struct RawLayer {
    pub(crate) levels: Levels,
    pub(crate) matte: bool,
}

pub(crate) struct PackedLayers {
    pub(crate) data: Vec<u8>,
    pub(crate) format: TextureFormat,
}

/// Layer 0 is the fallback, then `layers` in order.
pub(crate) fn pack_layers(layers: Vec<RawLayer>, formats: CompressedImageFormats) -> PackedLayers {
    if let Some(form) = passthrough_form(&layers, formats.contains(CompressedImageFormats::BC)) {
        let mut data = solid_block_chain(form, MISSING_TEXTURE_RGBA);
        for layer in &layers {
            for level in layer.levels.mips.iter().take(LAYER_MIPS as usize) {
                data.extend_from_slice(level);
            }
        }
        return PackedLayers {
            data,
            format: block_format(form),
        };
    }
    let mut data = solid_layer_chain(MISSING_TEXTURE_RGBA);
    for layer in layers {
        let mut chain = chain_to_layer(&layer.levels.into_rgba8());
        if layer.matte {
            chain.iter_mut().skip(3).step_by(4).for_each(|a| *a = 0);
        }
        data.extend_from_slice(&chain);
    }
    PackedLayers {
        data,
        format: TextureFormat::Rgba8Unorm,
    }
}

fn passthrough_form(layers: &[RawLayer], bc: bool) -> Option<BlpTexels> {
    if !bc {
        return None;
    }
    let form = layers.first()?.levels.texels;
    let uniform = form.is_block_compressed()
        && layers.iter().all(|l| {
            l.levels.texels == form
                && !l.matte
                && l.levels.width == LAYER_SIZE
                && l.levels.height == LAYER_SIZE
                && l.levels.mips.len() >= LAYER_MIPS as usize
                && l.levels.mips[..LAYER_MIPS as usize]
                    .iter()
                    .enumerate()
                    .all(|(i, level)| level.len() == level_bytes(form, i as u32))
        });
    uniform.then_some(form)
}

fn block_format(form: BlpTexels) -> TextureFormat {
    match form {
        BlpTexels::Bc1 => TextureFormat::Bc1RgbaUnorm,
        BlpTexels::Bc2 => TextureFormat::Bc2RgbaUnorm,
        BlpTexels::Bc3 => TextureFormat::Bc3RgbaUnorm,
        BlpTexels::Rgba8Unorm => TextureFormat::Rgba8Unorm,
    }
}

fn level_bytes(form: BlpTexels, level: u32) -> usize {
    let w = (LAYER_SIZE >> level).max(1);
    form.level_bytes(w, w)
}

/// Levels the BLP lacks are taken nearest from those it has, never averaged: averaging gamma-space
/// texels darkens them.
fn chain_to_layer(levels: &Levels) -> Vec<u8> {
    let mut chain = Vec::with_capacity(rgba_chain_bytes());
    for level in 0..LAYER_MIPS {
        let size = (LAYER_SIZE >> level).max(1);
        let src = if levels.width >= size {
            let ratio = (levels.width / size).max(1);
            (ratio.trailing_zeros() as usize).min(levels.mips.len() - 1)
        } else {
            0
        };
        let (sw, sh) = levels.size(src);
        extend_nearest(&mut chain, &levels.mips[src], (sw, sh), size);
    }
    chain
}

fn extend_nearest(out: &mut Vec<u8>, src: &[u8], (sw, sh): (u32, u32), size: u32) {
    if (sw, sh) == (size, size) {
        out.extend_from_slice(src);
        return;
    }
    let Some(buf) = image::RgbaImage::from_raw(sw, sh, src.to_vec()) else {
        out.extend(std::iter::repeat_n(0u8, (size * size * 4) as usize));
        return;
    };
    let resized = image::imageops::resize(&buf, size, size, image::imageops::FilterType::Nearest);
    out.extend_from_slice(resized.as_raw());
}

fn rgba_chain_bytes() -> usize {
    (0..LAYER_MIPS)
        .map(|i| ((LAYER_SIZE >> i).max(1).pow(2) * 4) as usize)
        .sum()
}

fn solid_layer_chain(rgba: [u8; 4]) -> Vec<u8> {
    rgba.repeat(rgba_chain_bytes() / 4)
}

/// A 4×4 block of one colour: RGB565 endpoints, every index 0.
fn solid_block(form: BlpTexels, rgba: [u8; 4]) -> Vec<u8> {
    let c = rgb565(rgba);
    let mut block = match form {
        BlpTexels::Bc2 => vec![(rgba[3] >> 4) * 0x11; 8],
        BlpTexels::Bc3 => vec![rgba[3], rgba[3], 0, 0, 0, 0, 0, 0],
        BlpTexels::Bc1 | BlpTexels::Rgba8Unorm => Vec::new(),
    };
    block.extend_from_slice(&[c as u8, (c >> 8) as u8, 0, 0, 0, 0, 0, 0]);
    block
}

fn rgb565(rgba: [u8; 4]) -> u16 {
    let [r, g, b] = [rgba[0], rgba[1], rgba[2]].map(u16::from);
    ((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)
}

fn solid_block_chain(form: BlpTexels, rgba: [u8; 4]) -> Vec<u8> {
    let block = solid_block(form, rgba);
    let blocks: usize = (0..LAYER_MIPS)
        .map(|i| level_bytes(form, i) / block.len())
        .sum();
    block.repeat(blocks)
}

/// Trilinear, anisotropy off: how the client filters ground textures on a fresh install.
pub(crate) fn layer_array(layer_count: u32, packed: PackedLayers) -> Image {
    let mut image = array_image(LAYER_SIZE, layer_count, packed.format);
    image.data = Some(packed.data);
    image.texture_descriptor.mip_level_count = LAYER_MIPS;
    image.data_order = TextureDataOrder::LayerMajor;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::default()
    });
    image
}

pub(crate) fn alpha_array(layer_count: u32, rgba: Vec<u8>) -> Image {
    let mut image = array_image(
        terrain::ALPHA_MAP_SIZE,
        layer_count,
        TextureFormat::Rgba8Unorm,
    );
    image.data = Some(rgba);
    image
}

pub(crate) fn shadow_array(layer_count: u32, texels: Vec<u8>) -> Image {
    let mut image = array_image(
        terrain::SHADOW_MAP_SIZE,
        layer_count,
        TextureFormat::R8Unorm,
    );
    image.data = Some(texels);
    image
}

fn array_image(size: u32, layer_count: u32, format: TextureFormat) -> Image {
    let mut image = Image::new_uninit(
        Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: layer_count,
        },
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
        ..TextureViewDescriptor::default()
    });
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(texels: BlpTexels, size: u32, count: u32, fill: u8) -> Levels {
        Levels {
            width: size,
            height: size,
            texels,
            mips: (0..count)
                .map(|i| {
                    let w = (size >> i).max(1);
                    vec![fill; texels.level_bytes(w, w)]
                })
                .collect(),
        }
    }

    fn raw(texels: BlpTexels, size: u32, count: u32, matte: bool) -> RawLayer {
        RawLayer {
            levels: levels(texels, size, count, 0),
            matte,
        }
    }

    #[test]
    fn blocks_pass_through_only_when_every_layer_allows() {
        let ok = || {
            vec![
                raw(BlpTexels::Bc2, 256, 8, false),
                raw(BlpTexels::Bc2, 256, 8, false),
            ]
        };
        assert_eq!(passthrough_form(&ok(), true), Some(BlpTexels::Bc2));
        assert_eq!(passthrough_form(&ok(), false), None);
        let mixed = vec![
            raw(BlpTexels::Bc1, 256, 8, false),
            raw(BlpTexels::Bc2, 256, 8, false),
        ];
        for refused in [
            mixed,
            vec![raw(BlpTexels::Bc2, 128, 7, false)],
            vec![raw(BlpTexels::Bc2, 256, 5, false)],
            vec![raw(BlpTexels::Bc2, 256, 8, true)],
            vec![raw(BlpTexels::Rgba8Unorm, 256, 8, false)],
            Vec::new(),
        ] {
            assert_eq!(passthrough_form(&refused, true), None);
        }
    }

    #[test]
    fn a_block_array_is_the_size_its_format_implies() {
        for form in [BlpTexels::Bc1, BlpTexels::Bc2, BlpTexels::Bc3] {
            let packed = pack_layers(
                vec![raw(form, 256, 8, false), raw(form, 256, 8, false)],
                CompressedImageFormats::BC,
            );
            assert_eq!(packed.format, block_format(form));
            let per_layer: usize = (0..LAYER_MIPS).map(|i| level_bytes(form, i)).sum();
            assert_eq!(packed.data.len(), 3 * per_layer, "{form:?}");
        }
    }

    #[test]
    fn the_fallback_block_decodes_to_its_colour() {
        for form in [BlpTexels::Bc1, BlpTexels::Bc2, BlpTexels::Bc3] {
            let top = blp::decode_level(form, 4, 4, &solid_block(form, MISSING_TEXTURE_RGBA));
            for px in top.as_chunks::<4>().0 {
                for (got, want) in px[..3].iter().zip(&MISSING_TEXTURE_RGBA[..3]) {
                    assert!(got.abs_diff(*want) <= 8, "{form:?}: {got} vs {want}");
                }
                assert!(
                    form == BlpTexels::Bc1 || px[3] == MISSING_TEXTURE_RGBA[3],
                    "{form:?}"
                );
            }
        }
    }

    #[test]
    fn decoded_layers_keep_their_bytes_and_matte_ones_lose_their_sheen() {
        let small = RawLayer {
            levels: levels(BlpTexels::Rgba8Unorm, 64, 7, 200),
            matte: true,
        };
        let full = RawLayer {
            levels: levels(BlpTexels::Rgba8Unorm, 256, 8, 200),
            matte: false,
        };
        let packed = pack_layers(vec![small, full], CompressedImageFormats::BC);
        assert_eq!(packed.format, TextureFormat::Rgba8Unorm);
        let per_layer = rgba_chain_bytes();
        assert_eq!(packed.data.len(), 3 * per_layer);
        let (matte, sheened) = packed.data[per_layer..].split_at(per_layer);
        for (i, (&m, &s)) in matte.iter().zip(sheened).enumerate() {
            let alpha = i % 4 == 3;
            assert_eq!((m, s), (if alpha { 0 } else { 200 }, 200), "byte {i}");
        }
    }
}
