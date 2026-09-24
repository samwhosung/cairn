use bevy::asset::RenderAssetUsages;
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, Image, ImageAddressMode, ImageFilterMode,
    ImageSampler, ImageSamplerDescriptor,
};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::flare::Glare;
use super::materials::{
    CelestialExtension, CelestialMaterial, DISC_HORIZON_FADE, DiscSpan, SpriteLook, StarExtension,
    StarMaterial,
};
use super::{Body, Celestial, StarPatch};
use crate::coords::wow_to_bevy;
use crate::sky_order;
use crate::skybox::ReplacedByPaintedSky;
use crate::source::{Install, Repeat};
use crate::texture::blp_image;

const STARS: &str = "Environments\\Stars\\Stars.m2";
const WHITE_STARS: &str = "Environments\\Stars\\Stars.blp";
const BLUE_STARS: &str = "Environments\\Stars\\Stars2.blp";

fn quad_mesh() -> Mesh {
    let mut quad = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    quad.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-0.5, -0.5, 0.0],
            [0.5, -0.5, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, 0.5, 0.0],
        ],
    );
    quad.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
    );
    quad.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    quad.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    quad
}

fn sprite(install: &Install, path: &str) -> Option<Image> {
    let decoded = install
        .0
        .read(path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| blp::decode(&bytes).map_err(|e| e.to_string()))
        .inspect_err(|e| warn!("no sky sprite {path}: {e}"))
        .ok()?;
    let top = decoded.mips.into_iter().next()?;
    let mut image = Image::new(
        Extent3d {
            width: top.width,
            height: top.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        top.rgba,
        // Filtered in linear light, as the client's sprites are.
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..ImageSamplerDescriptor::default()
    });
    Some(image)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_bodies(
    mut commands: Commands<'_, '_>,
    install: Res<'_, Install>,
    support: Option<Res<'_, CompressedImageFormatSupport>>,
    mut meshes: ResMut<'_, Assets<Mesh>>,
    mut images: ResMut<'_, Assets<Image>>,
    mut discs: ResMut<'_, Assets<CelestialMaterial>>,
    mut stars: ResMut<'_, Assets<StarMaterial>>,
) {
    let quad = meshes.add(quad_mesh());
    let bodies = [
        (
            Body::SunDisc,
            "Textures\\sunCenter.blp",
            sky_order::SUN_DISC_SORT_RUNG,
        ),
        (
            Body::SunGlare,
            "Textures\\sunGlare.blp",
            sky_order::GLARE_SORT_RUNG,
        ),
        (
            Body::MoonDisc,
            "textures\\moon.blp",
            sky_order::WHITE_MOON_SORT_RUNG,
        ),
        (
            Body::SecondMoon,
            "textures\\moon02.blp",
            sky_order::SECOND_MOON_SORT_RUNG,
        ),
        (
            Body::MoonGlare,
            "textures\\moonglare.blp",
            sky_order::GLARE_SORT_RUNG,
        ),
    ];
    for (body, path, rung) in bodies {
        let Some(texture) = sprite(&install, path) else {
            continue;
        };
        let glare = matches!(body, Body::SunGlare | Body::MoonGlare);
        // The second moon's colour is never set, so it draws black at alpha 0, and darkens the sky
        // only while it crosses the horizon, where the fade's ramp gives it alpha.
        let (color, alpha) = if body == Body::SecondMoon {
            (Color::BLACK, 0.0)
        } else {
            (Color::WHITE, 1.0)
        };
        let material = discs.add(CelestialMaterial {
            base: StandardMaterial {
                base_color: color,
                base_color_texture: Some(images.add(texture)),
                unlit: true,
                cull_mode: None,
                alpha_mode: if glare {
                    AlphaMode::Add
                } else {
                    AlphaMode::Premultiplied
                },
                depth_bias: rung,
                ..StandardMaterial::default()
            },
            extension: CelestialExtension {
                look: if glare {
                    SpriteLook {
                        horizon_slope: 0.0,
                        glare: 1,
                        disc_alpha: 0.0,
                    }
                } else {
                    SpriteLook {
                        horizon_slope: DISC_HORIZON_FADE,
                        glare: 0,
                        disc_alpha: alpha,
                    }
                },
                span: DiscSpan {
                    sin_bottom: 1.0,
                    sin_top: 1.0,
                },
            },
        });
        let mut sprite = commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(material),
            Transform::default(),
            Celestial(body),
        ));
        if glare {
            sprite.insert(Glare::default());
        } else {
            sprite.insert(ReplacedByPaintedSky);
        }
    }
    let formats = support.map_or(CompressedImageFormats::NONE, |s| s.0);
    spawn_stars(
        &mut commands,
        &install,
        formats,
        &mut meshes,
        &mut images,
        &mut stars,
    );
}

fn spawn_stars(
    commands: &mut Commands<'_, '_>,
    install: &Install,
    formats: CompressedImageFormats,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StarMaterial>,
) {
    let patches = install
        .0
        .read(STARS)
        .map_err(|e| e.to_string())
        .and_then(|b| model::parse_m2_render_submeshes(&b, "", &[]).map_err(|e| e.to_string()));
    let patches = match patches {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => return warn!("{STARS} has no batches: no stars"),
        Err(e) => return warn!("no stars: {e}"),
    };
    let mut texture = |path: &str| {
        let bytes = install.0.read(path).ok()?;
        let native = blp::decode_native(&bytes).ok()?;
        Some(images.add(blp_image(native, formats, Repeat::BOTH)))
    };
    let (white, blue) = (texture(WHITE_STARS), texture(BLUE_STARS));
    let radius = patches
        .iter()
        .flat_map(|p| &p.positions)
        .map(|p| Vec3::from(*p).length())
        .fold(0.0_f32, f32::max)
        .max(1e-3);
    for patch in &patches {
        let positions: Vec<[f32; 3]> = patch
            .positions
            .iter()
            .map(|p| (wow_to_bevy(*p) / radius).to_array())
            .collect();
        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, patch.uvs.clone());
        mesh.insert_indices(Indices::U32(patch.indices.clone()));
        let is_blue = patch
            .texture
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("stars2"));
        let authored_alpha = patch
            .alpha_anim
            .as_ref()
            .and_then(|a| a.seq(None).weight.as_ref())
            .map_or(1.0, |w| w.sample(0.0));
        let material = materials.add(StarMaterial {
            base: StandardMaterial {
                base_color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                base_color_texture: if is_blue { blue.clone() } else { white.clone() },
                unlit: true,
                cull_mode: None,
                alpha_mode: AlphaMode::Premultiplied,
                depth_bias: sky_order::STARS_SORT_RUNG,
                ..StandardMaterial::default()
            },
            extension: StarExtension {},
        });
        commands.spawn((
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::default(),
            StarPatch { authored_alpha },
            ReplacedByPaintedSky,
        ));
    }
}
