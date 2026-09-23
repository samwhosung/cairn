use std::collections::HashMap;
use std::num::NonZeroU16;

use bevy::asset::{AssetId, UntypedAssetId, embedded_asset};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
    MaterialPlugin,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, CompareFunction,
    Face, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use model::{FogPolicy, ModelBlend, WmoBatchClass};

pub type ModelMaterial = ExtendedMaterial<StandardMaterial, ModelExtension>;

const ALPHA_KEY: f32 = model::ALPHA_KEY_REF as f32 / 255.0;

/// Yards of transparent sort bias per authored batch, so one model's coplanar layers draw in
/// file order; capped under 1 so no batch index becomes its own pipeline.
const BATCH_ORDER_SORT_EPS: f32 = 1e-3;
const BATCH_ORDER_SORT_CAP: f32 = 0.9;

const NO_DEPTH_WRITE: u16 = 1;
const NO_DEPTH_TEST: u16 = 1 << 1;
const ADDITIVE: u16 = 1 << 2;
const OPAQUE_INTENT: u16 = 1 << 3;
const FOG_SHIFT: u16 = 4;
const MODULATE: u16 = 1 << 7;
const MODULATE_2X: u16 = 1 << 8;
const TWIN_CUTOUT: u16 = 1 << 10;
const ENV_MAP: u16 = 1 << 12;

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelKey {
    fade: bool,
    additive: bool,
    no_depth_write: bool,
    no_depth_test: bool,
    modulate: bool,
    modulate2x: bool,
}

impl From<&ModelExtension> for ModelKey {
    fn from(e: &ModelExtension) -> Self {
        let markers = e.clutter_fade.z as u16;
        Self {
            fade: e.model_flags.y > 0.5,
            additive: markers & ADDITIVE != 0,
            no_depth_write: markers & NO_DEPTH_WRITE != 0,
            no_depth_test: markers & NO_DEPTH_TEST != 0,
            modulate: markers & MODULATE != 0,
            modulate2x: markers & MODULATE_2X != 0,
        }
    }
}

/// Bevy packs these uniforms onto one binding in field order, as the shader's `ModelParams` lays
/// them out.
#[derive(Asset, AsBindGroup, Clone, TypePath)]
#[bind_group_data(ModelKey)]
pub struct ModelExtension {
    #[uniform(100)]
    pub clutter_fade: Vec4,
    #[uniform(100)]
    pub model_flags: Vec4,
    #[uniform(100)]
    pub sun_scale: Vec4,
    #[uniform(100)]
    pub tint: Vec4,
    #[uniform(100)]
    pub sidn: Vec4,
    #[uniform(100)]
    pub anim_slots: Vec4,
    #[storage(90, read_only, buffer, visibility(vertex, fragment))]
    pub light: Buffer,
}

impl MaterialExtension for ModelExtension {
    fn vertex_shader() -> ShaderRef {
        "embedded://world/model.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://world/model.wgsl".into()
    }

    /// The client writes depth for every model batch, transparent ones too, unless the batch says
    /// not to, and for a fading one always; the multiply and additive blends are not `AlphaMode`s.
    fn specialize(
        _pipeline: &MaterialExtensionPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let key = key.bind_group_data;
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = !key.no_depth_write || key.fade;
            if key.no_depth_test {
                ds.depth_compare = CompareFunction::Always;
            }
        }
        let target = descriptor
            .fragment
            .as_mut()
            .and_then(|f| f.targets.get_mut(0))
            .and_then(|t| t.as_mut());
        let Some(target) = target else {
            return Ok(());
        };
        let keep_alpha = BlendComponent {
            src_factor: BlendFactor::Zero,
            dst_factor: BlendFactor::One,
            operation: BlendOperation::Add,
        };
        if key.additive {
            target.blend = Some(BlendState {
                color: BlendComponent {
                    src_factor: BlendFactor::One,
                    dst_factor: BlendFactor::One,
                    operation: BlendOperation::Add,
                },
                alpha: keep_alpha,
            });
        }
        if key.modulate || key.modulate2x {
            target.blend = Some(BlendState {
                color: BlendComponent {
                    src_factor: BlendFactor::Dst,
                    dst_factor: if key.modulate2x {
                        BlendFactor::Src
                    } else {
                        BlendFactor::Zero
                    },
                    operation: BlendOperation::Add,
                },
                alpha: keep_alpha,
            });
        }
        Ok(())
    }
}

pub(crate) struct ModelMaterialPlugin;

impl Plugin for ModelMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "model.wgsl");
        app.add_plugins(MaterialPlugin::<ModelMaterial>::default())
            .init_resource::<ModelMaterials>();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum GroundShade {
    Lit,
    Shadowed,
}

impl GroundShade {
    fn selector(self) -> f32 {
        match self {
            GroundShade::Lit => 0.6,
            GroundShade::Shadowed => 0.2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    Steady,
    FadeTwin,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct BatchId {
    pub model: UntypedAssetId,
    pub index: usize,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone)]
pub(crate) struct BatchLook {
    pub texture: Option<Handle<Image>>,
    pub blend: ModelBlend,
    pub two_sided: bool,
    pub is_wmo: bool,
    pub interior: bool,
    pub emissive: bool,
    pub additive: bool,
    pub no_depth_write: bool,
    pub no_depth_test: bool,
    pub fog_policy: FogPolicy,
    pub env_map: bool,
    pub shade: GroundShade,
    pub batch_order: NonZeroU16,
    pub uv_offset_at_rest: [f32; 2],
    pub tint_at_rest: [f32; 3],
    pub animated: Option<BatchId>,
    pub wmo_class: Option<WmoBatchClass>,
    pub sidn: Option<[u8; 3]>,
    pub window: bool,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(PartialEq, Eq, Hash)]
struct MatKey {
    texture: Option<AssetId<Image>>,
    blend: ModelBlend,
    two_sided: bool,
    is_wmo: bool,
    interior: bool,
    emissive: bool,
    additive: bool,
    no_depth_write: bool,
    no_depth_test: bool,
    fog_policy: FogPolicy,
    env_map: bool,
    shade: GroundShade,
    batch_order: NonZeroU16,
    animated: Option<BatchId>,
    wmo_class: Option<WmoBatchClass>,
    sidn: Option<[u8; 3]>,
    window: bool,
    variant: Variant,
}

#[derive(Resource, Default)]
pub(crate) struct ModelMaterials(HashMap<MatKey, Handle<ModelMaterial>>);

impl ModelMaterials {
    pub(crate) fn get(
        &mut self,
        materials: &mut Assets<ModelMaterial>,
        look: &BatchLook,
        variant: Variant,
        light: &Buffer,
    ) -> Handle<ModelMaterial> {
        let key = MatKey {
            texture: look.texture.as_ref().map(Handle::id),
            blend: look.blend,
            two_sided: look.two_sided,
            is_wmo: look.is_wmo,
            interior: look.interior,
            emissive: look.emissive,
            additive: look.additive,
            no_depth_write: look.no_depth_write,
            no_depth_test: look.no_depth_test,
            fog_policy: look.fog_policy,
            env_map: look.env_map,
            shade: look.shade,
            batch_order: look.batch_order,
            animated: look.animated,
            wmo_class: look.wmo_class,
            sidn: look.sidn,
            window: look.window,
            variant,
        };
        self.0
            .entry(key)
            .or_insert_with(|| materials.add(build(look, variant, light)))
            .clone()
    }
}

fn build(look: &BatchLook, variant: Variant, light: &Buffer) -> ModelMaterial {
    let fade_variant = variant == Variant::FadeTwin;
    let blend = look.blend;
    let source_cutout = blend == ModelBlend::AlphaTest;
    let alpha_mode = if look.additive || fade_variant {
        AlphaMode::Blend
    } else {
        match blend {
            ModelBlend::Opaque => AlphaMode::Opaque,
            ModelBlend::AlphaTest => AlphaMode::Mask(ALPHA_KEY),
            ModelBlend::Blend | ModelBlend::Mod | ModelBlend::Mod2x => AlphaMode::Blend,
        }
    };
    let depth_bias = if matches!(alpha_mode, AlphaMode::Blend) {
        (f32::from(look.batch_order.get()) * BATCH_ORDER_SORT_EPS).min(BATCH_ORDER_SORT_CAP)
    } else {
        0.0
    };
    let opaque_intent = matches!(blend, ModelBlend::Opaque | ModelBlend::AlphaTest)
        && !fade_variant
        && !look.additive;
    let markers = (u16::from(look.no_depth_write) * NO_DEPTH_WRITE)
        | (u16::from(look.no_depth_test) * NO_DEPTH_TEST)
        | (u16::from(look.additive) * ADDITIVE)
        | (u16::from(opaque_intent) * OPAQUE_INTENT)
        | ((look.fog_policy as u16) << FOG_SHIFT)
        | (u16::from(blend == ModelBlend::Mod) * MODULATE)
        | (u16::from(blend == ModelBlend::Mod2x) * MODULATE_2X)
        | (u16::from(fade_variant && source_cutout) * TWIN_CUTOUT)
        | (u16::from(look.env_map) * ENV_MAP);
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let unlit =
        look.emissive || (!look.is_wmo && matches!(blend, ModelBlend::Mod | ModelBlend::Mod2x));
    let order = if look.is_wmo {
        f32::from(look.batch_order.get())
    } else {
        0.0
    };
    let class_lane = match (look.interior && look.is_wmo, look.wmo_class) {
        (true, Some(WmoBatchClass::Int)) => 1.0,
        (true, Some(WmoBatchClass::Trans)) => 2.0,
        _ => 0.0,
    };
    let sidn = look.sidn.unwrap_or([0; 3]).map(|c| f32::from(c) / 255.0);
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: look.texture.clone(),
            alpha_mode,
            double_sided: look.two_sided,
            cull_mode: if look.two_sided {
                None
            } else {
                Some(Face::Back)
            },
            depth_bias,
            ..StandardMaterial::default()
        },
        extension: ModelExtension {
            clutter_fade: Vec4::new(0.0, 0.0, f32::from(markers), 0.0),
            model_flags: Vec4::new(
                flag(look.is_wmo),
                flag(fade_variant),
                flag(look.interior),
                flag(unlit),
            ),
            sun_scale: Vec4::new(
                look.shade.selector(),
                order,
                look.uv_offset_at_rest[0],
                look.uv_offset_at_rest[1],
            ),
            tint: Vec4::new(
                look.tint_at_rest[0],
                look.tint_at_rest[1],
                look.tint_at_rest[2],
                class_lane,
            ),
            sidn: Vec4::new(sidn[0], sidn[1], sidn[2], flag(look.window)),
            anim_slots: Vec4::ZERO,
            light: light.clone(),
        },
    }
}
