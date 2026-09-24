// Texture bytes times the authored colour, multiplied in gamma space as the client does; the
// frame's decode pass turns the result linear once.

#import bevy_render::view::View

struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_start: f32,
    fog_end: f32,
    fog_unused: f32,
    farclip: f32,
};

@group(0) @binding(0) var<uniform> view: View;
@group(1) @binding(0) var effect_texture: texture_2d<f32>;
@group(1) @binding(1) var effect_sampler: sampler;
@group(1) @binding(2) var<storage, read> wow_light: WowLight;

struct EffectParams {
    fog_slot: vec4<f32>,
};
@group(1) @binding(3) var<uniform> effect_params: EffectParams;

const GREY: vec3<f32> = vec3<f32>(0.50196078, 0.50196078, 0.50196078);
const FOG_OFF: f32 = 0.0;
const FOG_BLACK: f32 = 2.0;
const FOG_WHITE: f32 = 3.0;
const FOG_GREY: f32 = 4.0;
const QUAD_NORMAL_UP: vec3<f32> = vec3<f32>(0.0, 1.0, 0.0);

struct Vertex {
    @location(0) camera_offset_or_world_position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) gamma_rgba: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) gamma_rgba: vec4<f32>,
    @location(2) view_depth: f32,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
#ifdef DECAL_WORLD_CLIP
    // A decal's depth has to tie with the ground it lies on within a small bias, so it goes
    // through the ground's own matrix.
    let world_position = v.camera_offset_or_world_position;
    out.clip_position = view.clip_from_world * vec4<f32>(world_position, 1.0);
    out.view_depth = -(view.view_from_world * vec4<f32>(world_position, 1.0)).z;
#else
    let view_pos = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    ) * v.camera_offset_or_world_position;
    out.clip_position = view.clip_from_view * vec4<f32>(view_pos, 1.0);
    out.view_depth = -view_pos.z;
#endif
    out.uv = v.uv;
    out.gamma_rgba = v.gamma_rgba;
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    if (wow_light.farclip > 0.0 && in.view_depth > wow_light.farclip) {
        discard;
    }
    let c = textureSampleBias(effect_texture, effect_sampler, in.uv, view.mip_bias) * in.gamma_rgba;
#ifdef BLEND_ALPHAKEY
    if (c.a < 224.0 / 255.0) {
        discard;
    }
#endif
    var rgb = c.rgb;
#ifdef EFFECT_LIT
    let L = -normalize(wow_light.light_sun.xyz);
    let lit = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(dot(QUAD_NORMAL_UP, L), 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    rgb = rgb * lit;
#endif
    let fog = effect_params.fog_slot.x;
    if (wow_light.fog_color.w > 0.5 && fog != FOG_OFF) {
        let denom = max(wow_light.fog_end - wow_light.fog_start, 0.001);
        let factor = clamp((wow_light.fog_end - in.view_depth) / denom, 0.0, 1.0);
        var fog_rgb = wow_light.fog_color.xyz;
        if (fog == FOG_BLACK) { fog_rgb = vec3<f32>(0.0); }
        else if (fog == FOG_WHITE) { fog_rgb = vec3<f32>(1.0); }
        else if (fog == FOG_GREY) { fog_rgb = GREY; }
        rgb = mix(fog_rgb, rgb, factor);
    }
#ifdef BLEND_ADD
    return vec4<f32>(rgb * c.a, 0.0);
#else
#ifdef BLEND_OPAQUE
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_ALPHAKEY
    return vec4<f32>(rgb, 1.0);
#else
#ifdef BLEND_MULTIPLY
    return vec4<f32>(rgb * c.a, c.a);
#else
#ifdef BLEND_MOD2X
    return vec4<f32>(rgb, 1.0);
#else
    return vec4<f32>(rgb, c.a);
#endif
#endif
#endif
#endif
#endif
}
