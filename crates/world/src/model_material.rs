use std::collections::HashMap;
use std::num::NonZeroU16;

use bevy::asset::{AssetId, UntypedAssetId, embedded_asset, load_embedded_asset};
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{
    ExtendedMaterial, MaterialExtension, MaterialExtensionKey, MaterialExtensionPipeline,
};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer, ColorWrites,
    CompareFunction, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use model::{FogPolicy, ModelBlend, WmoBatchClass};

use crate::draw_order::OrderedMaterialPlugin;
use crate::model::{ATTRIBUTE_WOW_JOINT_INDEX, ATTRIBUTE_WOW_JOINT_WEIGHT};
use crate::sky_order::{BAND_DROP, FAR_SIDE_SORT_RUNG};

pub type ModelMaterial = ExtendedMaterial<StandardMaterial, ModelExtension>;

const ALPHA_KEY: f32 = model::ALPHA_KEY_REF as f32 / 255.0;

/// Yards of transparent sort bias per authored batch, so one model's coplanar layers draw in
/// file order; capped under 1 so no batch index becomes its own pipeline.
const BATCH_ORDER_SORT_EPS: f32 = 1e-3;
const BATCH_ORDER_SORT_CAP: f32 = 0.9;
/// A depth-prime twin sorts this many yards ahead of its model's colour batches, so a fading body
/// primes its whole depth before any of it blends.
const DEPTH_PRIME_SORT_BIAS: f32 = -8.0;

const NO_DEPTH_WRITE: u16 = 1;
const NO_DEPTH_TEST: u16 = 1 << 1;
const ADDITIVE: u16 = 1 << 2;
const OPAQUE_INTENT: u16 = 1 << 3;
const FOG_SHIFT: u16 = 4;
const MODULATE: u16 = 1 << 7;
const MODULATE_2X: u16 = 1 << 8;
const DEPTH_PRIME: u16 = 1 << 9;
const TWIN_CUTOUT: u16 = 1 << 10;
const FAR_SIDE: u16 = 1 << 11;
const ENV_MAP: u16 = 1 << 12;
const SKY_DEPTH: u16 = 1 << 13;
const SIGHT: u16 = 1 << 14;

#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelKey {
    fade: bool,
    additive: bool,
    no_depth_write: bool,
    no_depth_test: bool,
    modulate: bool,
    modulate2x: bool,
    depth_prime: bool,
    sky_depth: bool,
    far_side: bool,
    sight: bool,
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
            depth_prime: markers & DEPTH_PRIME != 0,
            sky_depth: markers & SKY_DEPTH != 0,
            far_side: markers & FAR_SIDE != 0,
            sight: markers & SIGHT != 0,
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

impl ModelExtension {
    pub(crate) fn is_wmo(&self) -> bool {
        self.model_flags.x > 0.5
    }
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
        layout: &MeshVertexBufferLayoutRef,
        key: MaterialExtensionKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if layout.0.contains(ATTRIBUTE_WOW_JOINT_INDEX) {
            descriptor.vertex.shader_defs.push("WOW_RIG_SKIN".into());
            let mut attrs = Vec::with_capacity(7);
            for (attr, loc) in [
                (Mesh::ATTRIBUTE_POSITION, 0),
                (Mesh::ATTRIBUTE_NORMAL, 1),
                (Mesh::ATTRIBUTE_UV_0, 2),
                (Mesh::ATTRIBUTE_UV_1, 3),
                (Mesh::ATTRIBUTE_TANGENT, 4),
                (Mesh::ATTRIBUTE_COLOR, 5),
            ] {
                if layout.0.contains(attr) {
                    attrs.push(attr.at_shader_location(loc));
                }
            }
            attrs.push(ATTRIBUTE_WOW_JOINT_INDEX.at_shader_location(10));
            attrs.push(ATTRIBUTE_WOW_JOINT_WEIGHT.at_shader_location(11));
            descriptor.vertex.buffers = vec![layout.0.get_layout(&attrs)?];
        }
        let key = key.bind_group_data;
        if let Some(ds) = descriptor.depth_stencil.as_mut() {
            ds.depth_write_enabled = !key.no_depth_write || key.fade;
            if key.no_depth_test {
                ds.depth_compare = CompareFunction::Always;
            }
        }
        if key.far_side {
            crate::sky_order::sort_only(descriptor);
        }
        if key.sky_depth {
            descriptor.vertex.shader_defs.push("WOW_SKY_DEPTH".into());
            crate::sky_order::sky_pipeline_state(descriptor);
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
        if key.depth_prime {
            target.blend = None;
            target.write_mask = ColorWrites::empty();
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = true;
            }
        }
        if key.sight {
            target.blend = None;
            target.write_mask = ColorWrites::ALL;
            if let Some(ds) = descriptor.depth_stencil.as_mut() {
                ds.depth_write_enabled = true;
            }
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("WOW_SIGHT".into());
            }
        }
        Ok(())
    }
}

pub(crate) struct ModelMaterialPlugin;

/// The import both the model's and the terrain's shaders name for a sight frame: named in a
/// shader, an import must be loaded for any of its pipelines to build.
#[derive(Resource)]
struct SightShader(#[allow(dead_code)] Handle<Shader>);

impl Plugin for ModelMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "model.wgsl");
        embedded_asset!(app, "sight.wgsl");
        let sight = load_embedded_asset!(app, "sight.wgsl");
        app.add_plugins(OrderedMaterialPlugin::<ModelMaterial>::default())
            .insert_resource(SightShader(sight))
            .init_resource::<ModelMaterials>();
    }
}

/// Which sun intensity the model lane lights a batch with: the client's 1.0 on lit ground and 0.5
/// in the ground's baked shadow.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum GroundShade {
    Lit,
    Shadowed,
    Entity,
}

impl GroundShade {
    fn selector(self) -> f32 {
        match self {
            GroundShade::Lit => 0.6,
            GroundShade::Shadowed => 0.2,
            GroundShade::Entity => 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    Steady,
    FadeTwin,
    DepthPrime,
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
    pub batch_order: Option<NonZeroU16>,
    pub uv_offset_at_rest: [f32; 2],
    pub tint_at_rest: [f32; 3],
    pub animated: Option<BatchId>,
    pub seq_owner: Option<Entity>,
    pub wmo_class: Option<WmoBatchClass>,
    pub sidn: Option<[u8; 3]>,
    pub window: bool,
    pub skybox: bool,
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
    batch_order: Option<NonZeroU16>,
    animated: Option<BatchId>,
    seq_owner: Option<Entity>,
    wmo_class: Option<WmoBatchClass>,
    sidn: Option<[u8; 3]>,
    window: bool,
    skybox: bool,
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
            seq_owner: look.seq_owner,
            wmo_class: look.wmo_class,
            sidn: look.sidn,
            window: look.window,
            skybox: look.skybox,
            variant,
        };
        self.0
            .entry(key)
            .or_insert_with(|| materials.add(build(look, variant, light)))
            .clone()
    }
}

fn build(look: &BatchLook, variant: Variant, light: &Buffer) -> ModelMaterial {
    if variant == Variant::DepthPrime {
        return depth_prime(look, light);
    }
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
    let order = f32::from(look.batch_order.map_or(0, NonZeroU16::get));
    let depth_bias = match (alpha_mode, look.skybox) {
        (AlphaMode::Blend, false) => (order * BATCH_ORDER_SORT_EPS).min(BATCH_ORDER_SORT_CAP),
        (AlphaMode::Blend, true) => crate::sky_order::skybox_batch_bias(order),
        _ => 0.0,
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
        | (u16::from(look.env_map) * ENV_MAP)
        | (u16::from(look.skybox) * SKY_DEPTH);
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    let unlit =
        look.emissive || (!look.is_wmo && matches!(blend, ModelBlend::Mod | ModelBlend::Mod2x));
    let order = if look.is_wmo {
        f32::from(look.batch_order.map_or(0, NonZeroU16::get))
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

pub(crate) fn sight_twin_of(drawn: &ModelMaterial) -> ModelMaterial {
    let mut sight = drawn.clone();
    sight.extension.clutter_fade.z = f32::from(sight.extension.clutter_fade.z as u16 | SIGHT);
    sight
}

pub(crate) fn far_twin_of(near: &ModelMaterial) -> ModelMaterial {
    let mut far = near.clone();
    far.base.depth_bias += FAR_SIDE_SORT_RUNG - BAND_DROP;
    far.extension.clutter_fade.z = f32::from(far.extension.clutter_fade.z as u16 | FAR_SIDE);
    far
}

fn depth_prime(look: &BatchLook, light: &Buffer) -> ModelMaterial {
    let cutout = look.blend == ModelBlend::AlphaTest;
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            base_color_texture: look.texture.clone(),
            alpha_mode: AlphaMode::Blend,
            double_sided: look.two_sided,
            cull_mode: if look.two_sided {
                None
            } else {
                Some(Face::Back)
            },
            depth_bias: DEPTH_PRIME_SORT_BIAS,
            ..StandardMaterial::default()
        },
        extension: ModelExtension {
            clutter_fade: Vec4::new(
                0.0,
                0.0,
                f32::from(DEPTH_PRIME | if cutout { TWIN_CUTOUT } else { 0 }),
                0.0,
            ),
            model_flags: Vec4::ZERO,
            sun_scale: Vec4::new(GroundShade::Entity.selector(), 0.0, 0.0, 0.0),
            tint: Vec4::new(1.0, 1.0, 1.0, 0.0),
            sidn: Vec4::ZERO,
            anim_slots: Vec4::ZERO,
            light: light.clone(),
        },
    }
}
