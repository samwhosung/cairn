use std::ops::Range;

use bevy::asset::{AssetEvent, AssetId, load_embedded_asset};
use bevy::core_pipeline::core_3d::{CORE_3D_DEPTH_FORMAT, Transparent3d};
use bevy::ecs::system::SystemParamItem;
use bevy::ecs::system::lifetimeless::{Read, SRes};
use bevy::image::BevyDefault as _;
use bevy::mesh::VertexBufferLayout;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_phase::{
    AddRenderCommand, DrawFunctions, PhaseItem, PhaseItemExtraIndex, RenderCommand,
    RenderCommandResult, SetItemPipeline, TrackedRenderPass, ViewSortedRenderPhases,
};
use bevy::render::render_resource::binding_types::{
    sampler, storage_buffer_read_only_sized, texture_2d, uniform_buffer,
};
use bevy::render::render_resource::{
    BindGroup, BindGroupEntries, BindGroupLayoutDescriptor, BindGroupLayoutEntries, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferId, BufferUsages,
    CachedRenderPipelineId, ColorTargetState, ColorWrites, CompareFunction, DepthBiasState,
    DepthStencilState, DynamicUniformBuffer, FragmentState, IndexFormat, MultisampleState,
    PipelineCache, PrimitiveState, RawBufferVec, RenderPipelineDescriptor, SamplerBindingType,
    ShaderStages, ShaderType, SpecializedRenderPipeline, SpecializedRenderPipelines, StencilState,
    TextureFormat, TextureSampleType, VertexFormat, VertexState, VertexStepMode,
};
use bevy::render::renderer::{RenderDevice, RenderQueue};
use bevy::render::sync_world::MainEntity;
use bevy::render::texture::GpuImage;
use bevy::render::view::{
    ExtractedView, Msaa, ViewTarget, ViewUniform, ViewUniformOffset, ViewUniforms,
};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderStartup, RenderSystems};
use bevy::shader::Shader;

use super::stream::{EffectBlend, EffectFog, EffectQuads, EffectTopology, EffectVertex};
use crate::light::LightBuffer;

struct ExtractedDraw {
    cam: Entity,
    main_entity: Entity,
    texture: AssetId<Image>,
    blend: EffectBlend,
    topology: EffectTopology,
    fog: EffectFog,
    lit: bool,
    sort_anchor: Vec3,
    sort_bias: f32,
    raster_bias: i32,
    raster_slope: f32,
    cam_relative: bool,
    no_depth_test: bool,
    range: Range<u32>,
}

struct MergedDraw {
    index_range: Range<u32>,
    texture: AssetId<Image>,
    params_offset: u32,
}

#[derive(Resource)]
struct EffectMeta {
    vertices: RawBufferVec<EffectVertex>,
    indices: RawBufferVec<u32>,
    draws: Vec<ExtractedDraw>,
    merged: Vec<MergedDraw>,
    view_bind_group: Option<BindGroup>,
    params: DynamicUniformBuffer<EffectParams>,
    params_offsets: [u32; FOG_ROWS],
    params_buffer: Option<BufferId>,
}

const FOG_ROWS: usize = 5;

#[derive(Clone, Copy, ShaderType)]
struct EffectParams {
    fog_slot: Vec4,
}

impl Default for EffectMeta {
    fn default() -> Self {
        Self {
            vertices: RawBufferVec::new(BufferUsages::VERTEX),
            indices: RawBufferVec::new(BufferUsages::INDEX),
            draws: Vec::new(),
            merged: Vec::new(),
            view_bind_group: None,
            params: DynamicUniformBuffer::default(),
            params_offsets: [0; FOG_ROWS],
            params_buffer: None,
        }
    }
}

impl EffectMeta {
    fn write_params(&mut self, device: &RenderDevice, queue: &RenderQueue) {
        self.params.clear();
        for (slot, offset) in self.params_offsets.iter_mut().enumerate() {
            *offset = self.params.push(&EffectParams {
                fog_slot: Vec4::new(slot as f32, 0.0, 0.0, 0.0),
            });
        }
        self.params.write_buffer(device, queue);
        self.params_buffer = self.params.buffer().map(Buffer::id);
    }
}

#[derive(Resource, Default)]
struct EffectBindGroups {
    images: HashMap<AssetId<Image>, BindGroup>,
    params_buffer: Option<BufferId>,
}

#[derive(Resource)]
struct EffectPipeline {
    view_layout: BindGroupLayoutDescriptor,
    image_layout: BindGroupLayoutDescriptor,
    shader: Handle<Shader>,
}

fn init_effect_pipeline(mut commands: Commands<'_, '_>, server: Res<'_, AssetServer>) {
    let view_layout = BindGroupLayoutDescriptor::new(
        "effect_view_layout",
        &BindGroupLayoutEntries::single(
            ShaderStages::VERTEX_FRAGMENT,
            uniform_buffer::<ViewUniform>(true),
        ),
    );
    let image_layout = BindGroupLayoutDescriptor::new(
        "effect_image_layout",
        &BindGroupLayoutEntries::sequential(
            ShaderStages::FRAGMENT,
            (
                texture_2d(TextureSampleType::Float { filterable: true }),
                sampler(SamplerBindingType::Filtering),
                storage_buffer_read_only_sized(false, None),
                uniform_buffer::<EffectParams>(true),
            ),
        ),
    );
    commands.insert_resource(EffectPipeline {
        view_layout,
        image_layout,
        shader: load_embedded_asset!(server.as_ref(), "effect.wgsl"),
    });
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct EffectPipelineKey {
    samples: u32,
    hdr: bool,
    blend: EffectBlend,
    raster_bias: i32,
    raster_slope_bits: u32,
    lit: bool,
    no_depth_test: bool,
}

impl SpecializedRenderPipeline for EffectPipeline {
    type Key = EffectPipelineKey;

    #[allow(clippy::too_many_lines)]
    fn specialize(&self, key: Self::Key) -> RenderPipelineDescriptor {
        let vertex_layout = VertexBufferLayout::from_vertex_formats(
            VertexStepMode::Vertex,
            vec![
                VertexFormat::Float32x3,
                VertexFormat::Float32x2,
                VertexFormat::Float32x4,
            ],
        );
        let (blend, depth_write, blend_def) = match key.blend {
            EffectBlend::Add => (
                Some(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                false,
                "BLEND_ADD",
            ),
            EffectBlend::Alpha => (Some(BlendState::ALPHA_BLENDING), false, "BLEND_ALPHA"),
            EffectBlend::Opaque => (None, true, "BLEND_OPAQUE"),
            EffectBlend::AlphaKey => (None, true, "BLEND_ALPHAKEY"),
            EffectBlend::Multiply => (
                Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Dst,
                        dst_factor: BlendFactor::OneMinusSrcAlpha,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent::OVER,
                }),
                false,
                "BLEND_MULTIPLY",
            ),
            EffectBlend::Mod2x => (
                Some(BlendState {
                    color: BlendComponent {
                        src_factor: BlendFactor::Dst,
                        dst_factor: BlendFactor::Src,
                        operation: BlendOperation::Add,
                    },
                    alpha: BlendComponent {
                        src_factor: BlendFactor::Zero,
                        dst_factor: BlendFactor::One,
                        operation: BlendOperation::Add,
                    },
                }),
                false,
                "BLEND_MOD2X",
            ),
        };
        let mut shader_defs = vec![blend_def.into()];
        if key.raster_bias != 0 {
            shader_defs.push("DECAL_WORLD_CLIP".into());
        }
        if key.lit {
            shader_defs.push("EFFECT_LIT".into());
        }
        let depth_compare = if key.no_depth_test {
            CompareFunction::Always
        } else {
            CompareFunction::GreaterEqual
        };
        RenderPipelineDescriptor {
            label: Some("effect_pipeline".into()),
            layout: vec![self.view_layout.clone(), self.image_layout.clone()],
            vertex: VertexState {
                shader: self.shader.clone(),
                shader_defs: shader_defs.clone(),
                buffers: vec![vertex_layout],
                ..default()
            },
            fragment: Some(FragmentState {
                shader: self.shader.clone(),
                shader_defs,
                targets: vec![Some(ColorTargetState {
                    format: if key.hdr {
                        ViewTarget::TEXTURE_FORMAT_HDR
                    } else {
                        TextureFormat::bevy_default()
                    },
                    blend,
                    write_mask: ColorWrites::ALL,
                })],
                ..default()
            }),
            primitive: PrimitiveState {
                cull_mode: None,
                ..default()
            },
            depth_stencil: Some(DepthStencilState {
                format: CORE_3D_DEPTH_FORMAT,
                depth_write_enabled: depth_write,
                depth_compare,
                stencil: StencilState::default(),
                bias: DepthBiasState {
                    constant: key.raster_bias,
                    slope_scale: f32::from_bits(key.raster_slope_bits),
                    clamp: 0.0,
                },
            }),
            multisample: MultisampleState {
                count: key.samples,
                ..default()
            },
            ..default()
        }
    }
}

fn extract_effects(
    mut meta: ResMut<'_, EffectMeta>,
    mut bind_groups: ResMut<'_, EffectBindGroups>,
    quads: Extract<'_, '_, Res<'_, EffectQuads>>,
    mut image_events: Extract<'_, '_, MessageReader<'_, '_, AssetEvent<Image>>>,
) {
    for event in image_events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::Removed { id }
            | AssetEvent::Unused { id } => {
                bind_groups.images.remove(id);
            }
            AssetEvent::LoadedWithDependencies { .. } => {}
        }
    }
    meta.vertices.clear();
    meta.vertices.extend(quads.verts.iter().copied());
    meta.draws.clear();
    meta.draws.extend(quads.draws.iter().map(|d| ExtractedDraw {
        cam: d.cam,
        main_entity: d.main_entity,
        texture: d.texture,
        blend: d.blend,
        topology: d.topology,
        fog: d.fog,
        lit: d.lit,
        sort_anchor: d.sort_anchor,
        sort_bias: d.sort_bias,
        raster_bias: d.raster_bias,
        raster_slope: d.raster_slope,
        cam_relative: d.cam_relative,
        no_depth_test: d.no_depth_test,
        range: d.range.clone(),
    }));
}

fn queue_effects(
    effect_pipeline: Res<'_, EffectPipeline>,
    mut pipelines: ResMut<'_, SpecializedRenderPipelines<EffectPipeline>>,
    pipeline_cache: Res<'_, PipelineCache>,
    draw_functions: Res<'_, DrawFunctions<Transparent3d>>,
    meta: Res<'_, EffectMeta>,
    mut phases: ResMut<'_, ViewSortedRenderPhases<Transparent3d>>,
    views: Query<'_, '_, (&ExtractedView, &Msaa)>,
) {
    if meta.draws.is_empty() {
        return;
    }
    let draw_function = draw_functions.read().id::<DrawEffects>();
    for (view, msaa) in &views {
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let rangefinder = view.rangefinder3d();
        for (i, draw) in meta.draws.iter().enumerate() {
            if view.retained_view_entity.main_entity != MainEntity::from(draw.cam) {
                continue;
            }
            let pipeline = pipelines.specialize(
                &pipeline_cache,
                &effect_pipeline,
                EffectPipelineKey {
                    samples: msaa.samples(),
                    hdr: view.hdr,
                    blend: draw.blend,
                    raster_bias: draw.raster_bias,
                    raster_slope_bits: draw.raster_slope.to_bits(),
                    lit: draw.lit,
                    no_depth_test: draw.no_depth_test,
                },
            );
            phase.add(Transparent3d {
                distance: rangefinder.distance(&draw.sort_anchor) + draw.sort_bias,
                pipeline,
                entity: (Entity::PLACEHOLDER, MainEntity::from(draw.main_entity)),
                draw_function,
                batch_range: (i as u32)..(i as u32 + 1),
                extra_index: PhaseItemExtraIndex::None,
                indexed: true,
            });
        }
    }
}

type RunKey = (CachedRenderPipelineId, AssetId<Image>, u32);

#[allow(clippy::too_many_lines)]
fn prepare_effects(
    device: Res<'_, RenderDevice>,
    queue: Res<'_, RenderQueue>,
    draw_functions: Res<'_, DrawFunctions<Transparent3d>>,
    mut meta: ResMut<'_, EffectMeta>,
    mut phases: ResMut<'_, ViewSortedRenderPhases<Transparent3d>>,
    views: Query<'_, '_, &ExtractedView>,
) {
    let meta = &mut *meta;
    let cams: HashMap<MainEntity, Vec3> = views
        .iter()
        .map(|v| {
            (
                v.retained_view_entity.main_entity,
                v.world_from_view.translation(),
            )
        })
        .collect();
    for draw in &meta.draws {
        if draw.raster_bias != 0 || draw.cam_relative {
            continue;
        }
        let Some(cam) = cams.get(&MainEntity::from(draw.cam)) else {
            continue;
        };
        let offset = cam.to_array();
        for v in &mut meta.vertices.values_mut()[draw.range.start as usize..draw.range.end as usize]
        {
            v.pos[0] -= offset[0];
            v.pos[1] -= offset[1];
            v.pos[2] -= offset[2];
        }
    }
    meta.vertices.write_buffer(&device, &queue);
    meta.write_params(&device, &queue);
    let effect_fn = draw_functions.read().id::<DrawEffects>();
    meta.indices.clear();
    meta.merged.clear();
    let mut walked: Vec<bevy::render::view::RetainedViewEntity> = Vec::new();
    for view in &views {
        if walked.contains(&view.retained_view_entity) {
            continue;
        }
        walked.push(view.retained_view_entity);
        let Some(phase) = phases.get_mut(&view.retained_view_entity) else {
            continue;
        };
        let mut open: Option<(usize, u32, RunKey)> = None;
        let close = |items: &mut Vec<Transparent3d>,
                     open: &mut Option<(usize, u32, RunKey)>,
                     merged_len: usize| {
            if let Some((item_idx, run_len, _)) = open.take() {
                let m = merged_len as u32 - 1;
                items[item_idx].batch_range = m..m + run_len;
            }
        };
        for i in 0..phase.items.len() {
            let item = &phase.items[i];
            if item.draw_function != effect_fn {
                close(&mut phase.items, &mut open, meta.merged.len());
                continue;
            }
            let (pipeline, draw_idx) = (item.pipeline, item.batch_range.start as usize);
            let Some(draw) = meta.draws.get(draw_idx) else {
                close(&mut phase.items, &mut open, meta.merged.len());
                phase.items[i].batch_range = 0..0;
                continue;
            };
            let index_start = meta.indices.len() as u32;
            match draw.topology {
                EffectTopology::Quads => {
                    let mut b = draw.range.start;
                    while b < draw.range.end {
                        for k in [b, b + 1, b + 2, b, b + 2, b + 3] {
                            meta.indices.push(k);
                        }
                        b += 4;
                    }
                }
                EffectTopology::Tris => {
                    for k in draw.range.clone() {
                        meta.indices.push(k);
                    }
                }
            }
            let index_end = meta.indices.len() as u32;
            let params_offset = meta.params_offsets[draw.fog.slot() as usize];
            let key: RunKey = (pipeline, draw.texture, params_offset);
            match &mut open {
                Some((_, run_len, open_key)) if *open_key == key => {
                    if let Some(last) = meta.merged.last_mut() {
                        last.index_range.end = index_end;
                    }
                    *run_len += 1;
                }
                _ => {
                    close(&mut phase.items, &mut open, meta.merged.len());
                    meta.merged.push(MergedDraw {
                        index_range: index_start..index_end,
                        texture: draw.texture,
                        params_offset,
                    });
                    open = Some((i, 1, key));
                }
            }
        }
        close(&mut phase.items, &mut open, meta.merged.len());
    }
    if !meta.indices.is_empty() {
        meta.indices.write_buffer(&device, &queue);
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_effect_bind_groups(
    device: Res<'_, RenderDevice>,
    pipeline_cache: Res<'_, PipelineCache>,
    pipeline: Res<'_, EffectPipeline>,
    view_uniforms: Res<'_, ViewUniforms>,
    gpu_images: Res<'_, RenderAssets<GpuImage>>,
    light: Option<Res<'_, LightBuffer>>,
    mut meta: ResMut<'_, EffectMeta>,
    mut bind_groups: ResMut<'_, EffectBindGroups>,
) {
    let Some(view_binding) = view_uniforms.uniforms.binding() else {
        return;
    };
    meta.view_bind_group = Some(device.create_bind_group(
        "effect_view_bind_group",
        &pipeline_cache.get_bind_group_layout(&pipeline.view_layout),
        &BindGroupEntries::single(view_binding),
    ));
    let Some(light) = light else { return };
    let Some(params_binding) = meta.params.binding() else {
        return;
    };
    if bind_groups.params_buffer != meta.params_buffer {
        bind_groups.images.clear();
        bind_groups.params_buffer = meta.params_buffer;
    }
    for draw in &meta.draws {
        if bind_groups.images.contains_key(&draw.texture) {
            continue;
        }
        let Some(image) = gpu_images.get(draw.texture) else {
            continue;
        };
        bind_groups.images.insert(
            draw.texture,
            device.create_bind_group(
                "effect_image_bind_group",
                &pipeline_cache.get_bind_group_layout(&pipeline.image_layout),
                &BindGroupEntries::sequential((
                    &image.texture_view,
                    &image.sampler,
                    light.0.as_entire_binding(),
                    params_binding.clone(),
                )),
            ),
        );
    }
}

type DrawEffects = (SetItemPipeline, SetEffectViewBindGroup<0>, DrawEffectBatch);

struct SetEffectViewBindGroup<const I: usize>;

impl<P: PhaseItem, const I: usize> RenderCommand<P> for SetEffectViewBindGroup<I> {
    type Param = SRes<EffectMeta>;
    type ViewQuery = Read<ViewUniformOffset>;
    type ItemQuery = ();

    fn render<'w>(
        _item: &P,
        view_uniform: &'w ViewUniformOffset,
        _entity: Option<()>,
        meta: SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let Some(view_bind_group) = meta.into_inner().view_bind_group.as_ref() else {
            return RenderCommandResult::Failure("effect view bind group not available");
        };
        pass.set_bind_group(I, view_bind_group, &[view_uniform.offset]);
        RenderCommandResult::Success
    }
}

struct DrawEffectBatch;

impl<P: PhaseItem> RenderCommand<P> for DrawEffectBatch {
    type Param = (SRes<EffectMeta>, SRes<EffectBindGroups>);
    type ViewQuery = ();
    type ItemQuery = ();

    fn render<'w>(
        item: &P,
        _view: (),
        _entity: Option<()>,
        (meta, bind_groups): SystemParamItem<'w, '_, Self::Param>,
        pass: &mut TrackedRenderPass<'w>,
    ) -> RenderCommandResult {
        let meta = meta.into_inner();
        let Some(draw) = meta.merged.get(item.batch_range().start as usize) else {
            return RenderCommandResult::Skip;
        };
        let Some(image_bind_group) = bind_groups.into_inner().images.get(&draw.texture) else {
            return RenderCommandResult::Skip;
        };
        let (Some(vertices), Some(indices)) = (meta.vertices.buffer(), meta.indices.buffer())
        else {
            return RenderCommandResult::Failure("effect lane buffers not available");
        };
        pass.set_bind_group(1, image_bind_group, &[draw.params_offset]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), IndexFormat::Uint32);
        pass.draw_indexed(draw.index_range.clone(), 0, 0..1);
        RenderCommandResult::Success
    }
}

pub(crate) struct EffectRenderPlugin;

impl Plugin for EffectRenderPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "effect.wgsl");
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else {
            return;
        };
        render_app
            .init_resource::<EffectMeta>()
            .init_resource::<EffectBindGroups>()
            .init_resource::<SpecializedRenderPipelines<EffectPipeline>>()
            .add_render_command::<Transparent3d, DrawEffects>()
            .add_systems(RenderStartup, init_effect_pipeline)
            .add_systems(ExtractSchedule, extract_effects)
            .add_systems(
                Render,
                (
                    queue_effects.in_set(RenderSystems::Queue),
                    prepare_effects.in_set(RenderSystems::PrepareResources),
                    prepare_effect_bind_groups.in_set(RenderSystems::PrepareBindGroups),
                ),
            );
    }
}
