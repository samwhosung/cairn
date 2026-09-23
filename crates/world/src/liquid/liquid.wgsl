// Every liquid surface, one arm per liquid renderer the client has, in gamma space.
//   Terrain water: a depth swatch from the zone's water colours on the first texture stage, the
//     animated sheet on the second, combined as rgb = primary·swatch + sheet.rgb +
//     (secondary + 0.25)·sheet.a, alpha = swatch.a.
//   Building water outdoors: one stage, rgb = primary + sheet.rgb + secondary·sheet.a, where
//     primary is the lit deep river colour; alpha from the authored per-vertex byte.
//   Building water indoors: unlit, rgb = the pool material's colour + sheet.rgb, alpha likewise
//     summed.
//   Magma and slime, terrain or building: the sheet is the opaque body, unlit but fogged.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var frames: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var frames_samp: sampler;

struct LiquidParams {
    // x magma or slime; y the ocean swatch; z fogs indoors; w the sheen's shininess.
    kind: vec4<f32>,
    // x the renderer: 0 terrain, 1 building outdoors, 2 building indoors.
    path: vec4<f32>,
    // y frame count; z scrolls; w the clock runs.
    anim: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> w: LiquidParams;

struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,     // xyz the direction sunlight travels
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,     // w > 0.5 enables fog
    fog_params: vec4<f32>,    // x fog start, y fog end, w the far-clip wall (0 disables it)
    _sh: array<vec4<f32>, 6>,
    _sh_c16: vec4<f32>,
    water_river: array<vec4<f32>, 2>,  // shallow, deep; w alpha
    water_ocean: array<vec4<f32>, 2>,
    _grade: vec4<f32>,
    wmo_fog_color: vec4<f32>,
    wmo_fog_params: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

const TAG_INTERIOR_FOG: u32 = 0x40000000u;
const FRAMES_PER_SECOND: f32 = 24.0;
const SCROLL_PERIOD: f32 = 10.0;
const SWATCH_ROWS: f32 = 64.0;
const OCEAN_LAST_ROW_VALUE: f32 = 0.9;

struct LiquidVsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) depth: f32,
    @location(4) secondary_vtx: vec3<f32>,
    @location(5) vcolor: vec4<f32>,
    @location(6) @interpolate(flat) room_fog: u32,
}

// The sun's Blinn highlight with a local viewer, per vertex. The lighting sun never sets, so the
// fixed-function N·L gate never closes on the flat up normal.
fn sun_sheen(world_normal: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let n = normalize(world_normal);
    let to_light = -normalize(wow_light.light_sun.xyz);
    let to_view = normalize(view.world_position.xyz - world_pos);
    let half_v = normalize(to_light + to_view);
    let ndoth = max(dot(n, half_v), 0.0);
    return wow_light.light_spec.rgb * pow(ndoth, max(w.kind.w, 1.0));
}

fn anim_time() -> f32 {
    return w.anim.w * globals.time;
}

fn frame_layer() -> i32 {
    return i32(floor(anim_time() * FRAMES_PER_SECOND) % max(w.anim.y, 1.0));
}

fn apply_scroll(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x, uv.y + w.anim.z * fract(anim_time() / SCROLL_PERIOD));
}

// Linear eye-Z fog in gamma space. An indoor pool fogs with its room while the room is on the
// interior fog chain.
fn apply_fog(rgb: vec3<f32>, world_pos: vec3<f32>, room_fog: u32) -> vec3<f32> {
    var fog_color = wow_light.fog_color;
    var fog_span = wow_light.fog_params.xy;
    if (w.kind.z > 0.5 && room_fog != 0u) {
        fog_color = wow_light.wmo_fog_color;
        fog_span = wow_light.wmo_fog_params.xy;
    }
    if (fog_color.w <= 0.5) {
        return rgb;
    }
    let eye_z = -(view.view_from_world * vec4<f32>(world_pos, 1.0)).z;
    let denom = max(fog_span.y - fog_span.x, 0.001);
    let factor = clamp((fog_span.y - eye_z) / denom, 0.0, 1.0);
    return mix(fog_color.xyz, rgb, factor);
}

@vertex
fn vertex(in: Vertex) -> LiquidVsOut {
    var out: LiquidVsOut;
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    out.world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(in.position, 1.0));
    out.clip_position = position_world_to_clip(out.world_position.xyz);
    out.world_normal = mesh_functions::mesh_normal_local_to_world(in.normal, in.instance_index);
    out.uv = in.uv;
#ifdef VERTEX_COLORS
    out.vcolor = in.color;
#else
    out.vcolor = vec4<f32>(1.0);
#endif
    out.depth = in.uv_b.x;
    out.secondary_vtx = sun_sheen(out.world_normal, out.world_position.xyz);
    out.room_fog = mesh_functions::get_tag(in.instance_index) & TAG_INTERIOR_FOG;
    return out;
}

// One row of the client's 64-row swatch: a byte-exact integer ramp from shallow toward deep that
// stops a step short of deep. The ocean's last row is darkened and opaque.
fn swatch_row(shallow: vec4<f32>, deep: vec4<f32>, i: f32, ocean: bool) -> vec4<f32> {
    let c0 = vec4<f32>(round(shallow.rgb * 255.0), floor(shallow.w * 255.0));
    let c1 = vec4<f32>(round(deep.rgb * 255.0), floor(deep.w * 255.0));
    let row = c0 + floor(i * (c1 - c0) / SWATCH_ROWS);
    if ocean && i >= SWATCH_ROWS - 1.0 {
        return vec4<f32>(floor(row.rgb * OCEAN_LAST_ROW_VALUE), 255.0) / 255.0;
    }
    return row / 255.0;
}

// The swatch sampled with linear filtering at depth coordinate `v`.
fn swatch_at(shallow: vec4<f32>, deep: vec4<f32>, v: f32, ocean: bool) -> vec4<f32> {
    let t = clamp(v * SWATCH_ROWS - 0.5, 0.0, SWATCH_ROWS - 1.0);
    let i0 = floor(t);
    return mix(
        swatch_row(shallow, deep, i0, ocean),
        swatch_row(shallow, deep, min(i0 + 1.0, SWATCH_ROWS - 1.0), ocean),
        t - i0,
    );
}

@fragment
fn fragment(in: LiquidVsOut) -> @location(0) vec4<f32> {
    if (wow_light.fog_params.w > 0.0) {
        let clip_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        if (clip_z > wow_light.fog_params.w) {
            discard;
        }
    }

    let detail = textureSampleBias(
        frames,
        frames_samp,
        apply_scroll(in.uv),
        frame_layer(),
        view.mip_bias,
    );

    if (w.kind.x > 0.5) {
        return vec4<f32>(apply_fog(detail.rgb, in.world_position.xyz, in.room_fog), 1.0);
    }

    let depth = clamp(in.depth, 0.0, 1.0);
    var shallow = wow_light.water_river[0];
    var deep = wow_light.water_river[1];
    if (w.kind.y > 0.5) {
        shallow = wow_light.water_ocean[0];
        deep = wow_light.water_ocean[1];
    }
    let vtx_alpha = mix(shallow.w, deep.w, depth);
    if (w.path.x > 1.5) {
        let body = clamp(in.vcolor.rgb + detail.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        return vec4<f32>(
            apply_fog(body, in.world_position.xyz, in.room_fog),
            clamp(vtx_alpha + detail.a, 0.0, 1.0),
        );
    }
    let n = normalize(in.world_normal);
    let to_light = -normalize(wow_light.light_sun.xyz);
    let primary = clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(dot(n, to_light), 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    if (w.path.x > 0.5) {
        let rgb = primary * deep.rgb + detail.rgb + in.secondary_vtx * detail.a;
        return vec4<f32>(apply_fog(rgb, in.world_position.xyz, in.room_fog), vtx_alpha);
    }
    let swatch = swatch_at(shallow, deep, depth, w.kind.y > 0.5);
    let rgb = primary * swatch.rgb + detail.rgb + (in.secondary_vtx + vec3<f32>(0.25)) * detail.a;
    return vec4<f32>(apply_fog(rgb, in.world_position.xyz, in.room_fog), swatch.w);
}
