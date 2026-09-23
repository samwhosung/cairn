//! World shaders light, blend and fog in gamma space, as the client does in its 8-bit
//! framebuffer. This pass clamps the frame as bytes would be and decodes it to linear, so the sRGB
//! target stores the gamma values themselves.

use bevy::asset::{embedded_asset, load_embedded_asset};
use bevy::core_pipeline::FullscreenShader;
use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::ecs::query::QueryItem;
use bevy::prelude::*;
use bevy::render::extract_component::ExtractComponentPlugin;
use bevy::render::render_graph::{
    NodeRunError, RenderGraphContext, RenderGraphExt, RenderLabel, ViewNode, ViewNodeRunner,
};
use bevy::render::render_resource::binding_types::texture_2d;
use bevy::render::render_resource::{
    BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, CachedRenderPipelineId,
    ColorTargetState, ColorWrites, FragmentState, PipelineCache, RenderPassDescriptor,
    RenderPipelineDescriptor, ShaderStages, SpecializedRenderPipeline, SpecializedRenderPipelines,
    TextureFormat, TextureSampleType,
};
use bevy::render::renderer::RenderContext;
use bevy::render::view::ViewTarget;
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

use crate::view::WorldCamera;

pub(crate) struct DecodePlugin;

impl Plugin for DecodePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "decode.wgsl");
        app.add_plugins(ExtractComponentPlugin::<WorldCamera>::default());
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<SpecializedRenderPipelines<DecodePipeline>>()
            .add_systems(RenderStartup, init_pipeline)
            .add_systems(Render, prepare_pipelines.in_set(RenderSystems::Prepare))
            .add_render_graph_node::<ViewNodeRunner<DecodeNode>>(Core3d, DecodeLabel)
            .add_render_graph_edges(
                Core3d,
                (
                    Node3d::Tonemapping,
                    DecodeLabel,
                    Node3d::EndMainPassPostProcessing,
                ),
            );
    }
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
struct DecodeLabel;

#[derive(Resource)]
struct DecodePipeline {
    layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
    fullscreen: FullscreenShader,
}

impl SpecializedRenderPipeline for DecodePipeline {
    type Key = TextureFormat;

    fn specialize(&self, format: TextureFormat) -> RenderPipelineDescriptor {
        RenderPipelineDescriptor {
            label: Some("gamma_decode".into()),
            layout: vec![self.layout.clone()],
            vertex: self.fullscreen.to_vertex_state(),
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
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

fn init_pipeline(
    mut commands: Commands<'_, '_>,
    fullscreen: Res<'_, FullscreenShader>,
    server: Res<'_, AssetServer>,
) {
    let layout = BindGroupLayoutDescriptor::new(
        "gamma_decode",
        &BindGroupLayoutEntries::single(
            ShaderStages::FRAGMENT,
            texture_2d(TextureSampleType::Float { filterable: false }),
        ),
    );
    commands.insert_resource(DecodePipeline {
        layout,
        shader: load_embedded_asset!(server.as_ref(), "decode.wgsl"),
        fullscreen: fullscreen.clone(),
    });
}

#[derive(Component)]
struct ViewDecodePipeline(CachedRenderPipelineId);

fn prepare_pipelines(
    mut commands: Commands<'_, '_>,
    mut cache: ResMut<'_, PipelineCache>,
    pipeline: Res<'_, DecodePipeline>,
    mut pipelines: ResMut<'_, SpecializedRenderPipelines<DecodePipeline>>,
    views: Query<'_, '_, (Entity, &ViewTarget), With<WorldCamera>>,
) {
    for (entity, target) in &views {
        let id = pipelines.specialize(&cache, &pipeline, target.out_texture_view_format());
        cache.block_on_render_pipeline(id);
        commands.entity(entity).insert(ViewDecodePipeline(id));
    }
}

#[derive(Default)]
struct DecodeNode;

impl ViewNode for DecodeNode {
    type ViewQuery = (&'static ViewTarget, &'static ViewDecodePipeline);

    fn run<'w>(
        &self,
        _graph: &mut RenderGraphContext<'_>,
        render_context: &mut RenderContext<'w>,
        (target, pipeline): QueryItem<'w, '_, Self::ViewQuery>,
        world: &'w World,
    ) -> Result<(), NodeRunError> {
        let cache = world.resource::<PipelineCache>();
        let Some(decode) = cache.get_render_pipeline(pipeline.0) else {
            return Ok(());
        };
        let layout = cache.get_bind_group_layout(&world.resource::<DecodePipeline>().layout);
        let bind_group = render_context.render_device().create_bind_group(
            "gamma_decode",
            &layout,
            &BindGroupEntries::single(target.main_texture_view()),
        );
        let mut pass = render_context
            .command_encoder()
            .begin_render_pass(&RenderPassDescriptor {
                label: Some("gamma_decode"),
                color_attachments: &[Some(
                    target.out_texture_color_attachment(Some(LinearRgba::NONE)),
                )],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        pass.set_pipeline(decode);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }
}
