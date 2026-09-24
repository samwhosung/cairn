//! World shaders light, blend and fog in gamma space, as the client does in its 8-bit
//! framebuffer. This pass adds the client's full-screen glow, clamps the frame as bytes would be
//! and decodes it to linear, so the sRGB target stores the gamma values themselves.

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::camera::ExtractedCamera;
use bevy::render::extract_component::ExtractComponentPlugin;
use bevy::render::extract_resource::{ExtractResource, ExtractResourcePlugin};
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_resource::binding_types::{sampler, texture_2d, uniform_buffer_sized};
use bevy::render::render_resource::{
    AddressMode, BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries,
    Buffer, BufferDescriptor, BufferUsages, CachedRenderPipelineId, ColorTargetState, ColorWrites,
    Extent3d, FilterMode, FragmentState, Operations, PipelineCache, RenderPassColorAttachment,
    RenderPassDescriptor, RenderPipelineDescriptor, Sampler, SamplerBindingType, SamplerDescriptor,
    ShaderStages, SpecializedRenderPipeline, SpecializedRenderPipelines, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
};
use bevy::render::renderer::{RenderContext, RenderDevice, RenderQueue};
use bevy::render::texture::{CachedTexture, TextureCache};
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::light::SceneLight;
use crate::view::WorldCamera;

/// Whether the world draws the client's full-screen glow; without it the frame is only decoded.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullScreenGlow(pub bool);

impl Default for FullScreenGlow {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Resource, Clone, Copy, Default, PartialEq, ExtractResource)]
struct GlowWeight(f32);

const QUARTER_MIN_SIDE: u32 = 8;
const UNIFORM_BYTES: u64 = 16;

pub(crate) struct GlowPlugin;

impl Plugin for GlowPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "glow.wgsl");
        app.init_resource::<FullScreenGlow>()
            .init_resource::<GlowWeight>()
            .add_plugins((
                ExtractComponentPlugin::<WorldCamera>::default(),
                ExtractResourcePlugin::<GlowWeight>::default(),
            ))
            .add_systems(PostUpdate, weigh_glow);
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<CombinePipeline>>()
            .add_systems(RenderStartup, init_pipelines)
            .add_systems(Render, prepare_views.in_set(RenderSystems::Prepare))
            .add_render_graph_node::<ViewNodeRunner<GlowNode>>(Core3d, GlowLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::Tonemapping,
                    GlowLabel,
                    Node3d::EndMainPassPostProcessing,
                ),
            );
    }
}

fn weigh_glow(
    light: Res<'_, SceneLight>,
    glow: Res<'_, FullScreenGlow>,
    mut weight: ResMut<'_, GlowWeight>,
) {
    let w = if glow.0 { light.glow } else { 0.0 };
    if weight.0.to_bits() != w.to_bits() {
        weight.0 = w;
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct GlowLabel;

#[derive(Resource)]
struct GlowPipelines {
    filter_layout: BindGroupLayoutDescriptor,
    sampler: Sampler,
    downsample: CachedRenderPipelineId,
    blur_across: CachedRenderPipelineId,
    blur_down: CachedRenderPipelineId,
}

#[derive(Resource)]
struct CombinePipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
}

impl SpecializedRenderPipeline for CombinePipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("glow_combine".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                entry_point: Some("combine".into()),
                targets: vec![Some(ColorTargetState {
                    format,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..FragmentState::default()
            }),
            ..RenderPipelineDescriptor::default()
        }
    }
}

fn init_pipelines(
    mut commands: Commands<'_, '_>,
    device: Res<'_, RenderDevice>,
    fullscreen: Res<'_, FullscreenShader>,
    server: Res<'_, AssetServer>,
    cache: Res<'_, PipelineCache>,
) {
    let shader: Handle<Shader> = load_embedded_asset!(server.as_ref(), "glow.wgsl");
    let filterable = TextureSampleType::Float { filterable: true };
    let filter_layout = BindGroupLayoutDescriptor::new(
        "glow_filter",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(filterable),
                sampler(SamplerBindingType::Filtering),
            ),
        ),
    );
    let combine_layout = BindGroupLayoutDescriptor::new(
        "glow_combine",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(filterable),
                sampler(SamplerBindingType::Filtering),
                texture_2d(filterable),
                uniform_buffer_sized(false, std::num::NonZero::new(UNIFORM_BYTES)),
            ),
        ),
    );
    let sampler = device.create_sampler(&SamplerDescriptor {
        min_filter: FilterMode::Linear,
        mag_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        ..SamplerDescriptor::default()
    });
    let filter = |label: &'static str, entry: &'static str| {
        cache.queue_render_pipeline(RenderPipelineDescriptor {
            label: Some(label.into()),
            layout: vec![filter_layout.clone()],
            vertex: fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: shader.clone(),
                entry_point: Some(entry.into()),
                targets: vec![Some(ColorTargetState {
                    format: ViewTarget::TEXTURE_FORMAT_HDR,
                    blend: None,
                    write_mask: ColorWrites::ALL,
                })],
                ..FragmentState::default()
            }),
            ..RenderPipelineDescriptor::default()
        })
    };
    commands.insert_resource(GlowPipelines {
        downsample: filter("glow_downsample", "downsample"),
        blur_across: filter("glow_blur_across", "blur_across"),
        blur_down: filter("glow_blur_down", "blur_down"),
        filter_layout,
        sampler,
    });
    commands.insert_resource(CombinePipeline {
        layout: combine_layout,
        shader,
        fullscreen: fullscreen.clone(),
    });
}

#[derive(Component)]
struct GlowTargets {
    combine: CachedRenderPipelineId,
    blurred: CachedTexture,
    blurred_across: CachedTexture,
    blur_across: BindGroup,
    blur_down: BindGroup,
    weight: Buffer,
}

type GlowViews<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static ViewTarget,
        &'static ExtractedCamera,
        Option<&'static GlowTargets>,
    ),
    With<WorldCamera>,
>;

#[allow(clippy::too_many_arguments)]
fn prepare_views(
    mut commands: Commands<'_, '_>,
    mut cache: ResMut<'_, PipelineCache>,
    mut textures: ResMut<'_, TextureCache>,
    device: Res<'_, RenderDevice>,
    pipelines: Res<'_, GlowPipelines>,
    combine: Res<'_, CombinePipeline>,
    mut specialized: ResMut<'_, SpecializedRenderPipelines<CombinePipeline>>,
    views: GlowViews<'_, '_>,
) {
    for (entity, target, camera, held) in &views {
        let Some(size) = camera.physical_viewport_size else {
            continue;
        };
        let id = specialized.specialize(&cache, &combine, target.out_texture_view_format());
        for pipeline in [
            id,
            pipelines.downsample,
            pipelines.blur_across,
            pipelines.blur_down,
        ] {
            cache.block_on_render_pipeline(pipeline);
        }
        let mut quarter = |label: &'static str| {
            textures.get(
                &device,
                TextureDescriptor {
                    label: Some(label),
                    size: Extent3d {
                        width: (size.x / 4).max(QUARTER_MIN_SIDE),
                        height: (size.y / 4).max(QUARTER_MIN_SIDE),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: TextureDimension::D2,
                    format: ViewTarget::TEXTURE_FORMAT_HDR,
                    usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
            )
        };
        let (blurred, blurred_across) = (quarter("glow_blurred"), quarter("glow_blurred_across"));
        if held.is_some_and(|h| {
            h.combine == id
                && h.blurred.texture.id() == blurred.texture.id()
                && h.blurred_across.texture.id() == blurred_across.texture.id()
        }) {
            continue;
        }
        let layout = cache.get_bind_group_layout(&pipelines.filter_layout);
        let blur_across = device.create_bind_group(
            "glow_blur_across",
            &layout,
            &BindGroupEntries::sequential((&blurred.default_view, &pipelines.sampler)),
        );
        let blur_down = device.create_bind_group(
            "glow_blur_down",
            &layout,
            &BindGroupEntries::sequential((&blurred_across.default_view, &pipelines.sampler)),
        );
        let weight = device.create_buffer(&BufferDescriptor {
            label: Some("glow_weight"),
            size: UNIFORM_BYTES,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        commands.entity(entity).insert(GlowTargets {
            combine: id,
            blurred,
            blurred_across,
            blur_across,
            blur_down,
            weight,
        });
    }
}

#[derive(Default)]
struct GlowNode;

impl ViewNode for GlowNode {
    type ViewQuery = (&'static ViewTarget, &'static GlowTargets);

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext<'_>,
        render_context: &mut RenderContext<'w>,
        (target, glow): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let cache = world.resource::<PipelineCache>();
        let pipelines = world.resource::<GlowPipelines>();
        let (Some(combine), Some(downsample), Some(across), Some(down)) = (
            cache.get_render_pipeline(glow.combine),
            cache.get_render_pipeline(pipelines.downsample),
            cache.get_render_pipeline(pipelines.blur_across),
            cache.get_render_pipeline(pipelines.blur_down),
        ) else {
            return Ok(());
        };
        let weight = world.resource::<GlowWeight>().0;
        let device = render_context.render_device().clone();
        let glow_shows = weight != 0.0;
        if glow_shows {
            let layout = cache.get_bind_group_layout(&pipelines.filter_layout);
            let frame = device.create_bind_group(
                "glow_downsample",
                &layout,
                &BindGroupEntries::sequential((target.main_texture_view(), &pipelines.sampler)),
            );
            for (label, pipeline, bind, dst) in [
                ("glow_downsample", downsample, &frame, &glow.blurred),
                (
                    "glow_blur_across",
                    across,
                    &glow.blur_across,
                    &glow.blurred_across,
                ),
                ("glow_blur_down", down, &glow.blur_down, &glow.blurred),
            ] {
                let mut pass =
                    render_context
                        .command_encoder()
                        .begin_render_pass(&RenderPassDescriptor {
                            label: Some(label),
                            color_attachments: &[Some(RenderPassColorAttachment {
                                view: &dst.default_view,
                                depth_slice: None,
                                resolve_target: None,
                                ops: Operations::default(),
                            })],
                            depth_stencil_attachment: None,
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        let uniform: Vec<u8> = [weight, 0.0, 0.0, 0.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        world
            .resource::<RenderQueue>()
            .write_buffer(&glow.weight, 0, &uniform);
        let layout = cache.get_bind_group_layout(&world.resource::<CombinePipeline>().layout);
        let bind_group = device.create_bind_group(
            "glow_combine",
            &layout,
            &BindGroupEntries::sequential((
                target.main_texture_view(),
                &pipelines.sampler,
                &glow.blurred.default_view,
                glow.weight.as_entire_binding(),
            )),
        );
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("glow_combine"),
                color_attachments: &[Some(
                    target.out_texture_color_attachment(Some(LinearRgba::NONE)),
                )],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        pass.set_pipeline(combine);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
