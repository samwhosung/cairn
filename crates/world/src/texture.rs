use std::io;

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages};
use bevy::image::{
    CompressedImageFormats, Image, ImageAddressMode, ImageFilterMode, ImageSampler,
    ImageSamplerDescriptor,
};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use blp::{BlpTexels, NativeBlp};

use crate::source::{Repeat, repeat_of};

#[derive(TypePath)]
pub(crate) struct BlpLoader {
    formats: CompressedImageFormats,
}

impl BlpLoader {
    pub(crate) fn new(formats: CompressedImageFormats) -> Self {
        Self { formats }
    }
}

impl AssetLoader for BlpLoader {
    type Asset = Image;
    type Settings = ();
    type Error = io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        (): &(),
        ctx: &mut LoadContext<'_>,
    ) -> Result<Image, io::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let blp = blp::decode_native(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let repeat = repeat_of(&ctx.path().path().to_string_lossy());
        Ok(blp_image(blp, self.formats, repeat))
    }

    fn extensions(&self) -> &[&str] {
        &["blp"]
    }
}

/// A texture as the GPU takes it, every level the file stores. DXT levels stay blocks when
/// `formats` has BC and mip 0 is whole 4x4 blocks, which a BC texture must be; otherwise every
/// level is RGBA8. No format is sRGB: shaders read the texel bytes as stored, the gamma-space
/// values WoW lights in.
pub fn blp_image(blp: NativeBlp, formats: CompressedImageFormats, repeat: Repeat) -> Image {
    let whole_blocks = blp.width.is_multiple_of(4) && blp.height.is_multiple_of(4);
    let as_blocks = formats.contains(CompressedImageFormats::BC) && whole_blocks;
    let levels = blp.mips.len() as u32;
    let (format, data) = match block_format(blp.texels) {
        Some(format) if as_blocks => (format, blp.mips.into_iter().flat_map(|m| m.bytes).collect()),
        _ => (
            TextureFormat::Rgba8Unorm,
            blp.mips
                .iter()
                .flat_map(|m| blp::decode_level(blp.texels, m.width, m.height, &m.bytes))
                .collect(),
        ),
    };
    let size = Extent3d {
        width: blp.width,
        height: blp.height,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_uninit(
        size,
        TextureDimension::D2,
        format,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    let address = |repeats| {
        if repeats {
            ImageAddressMode::Repeat
        } else {
            ImageAddressMode::ClampToEdge
        }
    };
    // Trilinear, anisotropy off: how the 1.12.1 client filters world textures on a fresh install.
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: address(repeat.u),
        address_mode_v: address(repeat.v),
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::default()
    });
    image
}

fn block_format(texels: BlpTexels) -> Option<TextureFormat> {
    match texels {
        BlpTexels::Rgba8Unorm => None,
        BlpTexels::Bc1 => Some(TextureFormat::Bc1RgbaUnorm),
        BlpTexels::Bc2 => Some(TextureFormat::Bc2RgbaUnorm),
        BlpTexels::Bc3 => Some(TextureFormat::Bc3RgbaUnorm),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_level_dxt1_blp(width: u32, height: u32, block: [u8; 8]) -> Vec<u8> {
        let texels_at = 148 + 1024;
        let mut file = vec![0u8; texels_at];
        file[..4].copy_from_slice(b"BLP2");
        file[4..8].copy_from_slice(&1u32.to_le_bytes());
        file[8] = 2;
        file[12..16].copy_from_slice(&width.to_le_bytes());
        file[16..20].copy_from_slice(&height.to_le_bytes());
        let len = BlpTexels::Bc1.level_bytes(width, height);
        file[20..24].copy_from_slice(&(texels_at as u32).to_le_bytes());
        file[84..88].copy_from_slice(&(len as u32).to_le_bytes());
        file.extend(block.iter().cycle().take(len));
        file
    }

    const RED_BLOCK: [u8; 8] = [0x00, 0xF8, 0x1F, 0x00, 0x00, 0x00, 0x00, 0x00];

    fn red(width: u32, height: u32) -> NativeBlp {
        blp::decode_native(&one_level_dxt1_blp(width, height, RED_BLOCK)).expect("decodes")
    }

    #[test]
    fn blocks_pass_through_when_the_gpu_takes_them() {
        let image = blp_image(red(8, 8), CompressedImageFormats::BC, Repeat::BOTH);
        assert_eq!(image.texture_descriptor.format, TextureFormat::Bc1RgbaUnorm);
        assert_eq!(image.data, Some(RED_BLOCK.repeat(4)));
    }

    #[test]
    fn blocks_decode_without_bc_or_off_the_block_grid() {
        for (width, height, formats) in [
            (8, 8, CompressedImageFormats::NONE),
            (8, 8, CompressedImageFormats::ASTC_LDR),
            (6, 6, CompressedImageFormats::BC),
            (8, 6, CompressedImageFormats::BC),
        ] {
            let image = blp_image(red(width, height), formats, Repeat::BOTH);
            assert_eq!(image.texture_descriptor.format, TextureFormat::Rgba8Unorm);
            let data = image.data.expect("pixels");
            assert_eq!(data.len(), (width * height * 4) as usize);
            assert!(
                data.as_chunks::<4>()
                    .0
                    .iter()
                    .all(|px| *px == [255, 0, 0, 255])
            );
        }
    }

    #[test]
    fn the_sampler_repeats_where_asked() {
        let repeat = Repeat { u: true, v: false };
        let image = blp_image(red(8, 8), CompressedImageFormats::BC, repeat);
        let ImageSampler::Descriptor(sampler) = image.sampler else {
            panic!("the loader sets its own sampler");
        };
        assert_eq!(sampler.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(sampler.address_mode_v, ImageAddressMode::ClampToEdge);
        assert_eq!(sampler.mipmap_filter, ImageFilterMode::Linear);
        assert_eq!(sampler.anisotropy_clamp, 1);
    }
}
