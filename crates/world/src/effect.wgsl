// Effect quads: texture times vertex colour in gamma space, unlit, fogged as the draw asks.
// Additive draws premultiply by alpha and write zero alpha, so the blend adds them.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    mesh_view_bindings::view,
    view_transformations::position_world_to_clip,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var effect_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var effect_sampler: sampler;

struct EffectParams {
    // x fogs with the scene; y adds; z positions are relative to the camera.
    params: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> e: EffectParams;

struct WowLight {
    _light: array<vec4<f32>, 4>,
    fog_color: vec4<f32>,     // w > 0.5 enables fog
    fog_params: vec4<f32>,    // x fog start, y fog end, w the far-clip wall (0 disables it)
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

struct EffectVsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) view_z: f32,
}

@vertex
fn vertex(in: Vertex) -> EffectVsOut {
    var out: EffectVsOut;
    if (e.params.z > 0.5) {
        // Only the view's rotation applies: the camera's own position is already taken out.
        let view_pos = mat3x3<f32>(
            view.view_from_world[0].xyz,
            view.view_from_world[1].xyz,
            view.view_from_world[2].xyz,
        ) * in.position;
        out.clip_position = view.clip_from_view * vec4<f32>(view_pos, 1.0);
        out.view_z = -view_pos.z;
    } else {
        let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
        let world = mesh_functions::mesh_position_local_to_world(
            world_from_local,
            vec4<f32>(in.position, 1.0),
        );
        out.clip_position = position_world_to_clip(world.xyz);
        out.view_z = -(view.view_from_world * world).z;
    }
    out.uv = in.uv;
#ifdef VERTEX_COLORS
    out.color = in.color;
#else
    out.color = vec4<f32>(1.0);
#endif
    return out;
}

@fragment
fn fragment(in: EffectVsOut) -> @location(0) vec4<f32> {
    if (wow_light.fog_params.w > 0.0 && in.view_z > wow_light.fog_params.w) {
        discard;
    }
    let c = textureSampleBias(effect_texture, effect_sampler, in.uv, view.mip_bias) * in.color;
    var rgb = c.rgb;
    if (wow_light.fog_color.w > 0.5 && e.params.x > 0.5) {
        let denom = max(wow_light.fog_params.y - wow_light.fog_params.x, 0.001);
        let factor = clamp((wow_light.fog_params.y - in.view_z) / denom, 0.0, 1.0);
        rgb = mix(wow_light.fog_color.xyz, rgb, factor);
    }
    if (e.params.y > 0.5) {
        return vec4<f32>(rgb * c.a, 0.0);
    }
    return vec4<f32>(rgb, c.a);
}
