#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{view, globals},
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var frames: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var frames_samp: sampler;

struct LiquidParams {
    fullbright: f32,
    ocean: f32,
    room_fogged: f32,
    shininess: f32,
    renderer: f32,
    frame_count: f32,
    scrolls: f32,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var<uniform> w: LiquidParams;

struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    sun_travel: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec3<f32>,
    fog_on: f32,
    fog_start: f32,
    fog_end: f32,
    _fog_unused: f32,
    far_wall: f32,
    _sh: array<vec4<f32>, 6>,
    _sh_c16: vec4<f32>,
    river_shallow: vec4<f32>,
    river_deep: vec4<f32>,
    ocean_shallow: vec4<f32>,
    ocean_deep: vec4<f32>,
    _grade: vec4<f32>,
    room_fog_color: vec3<f32>,
    room_fog_on: f32,
    room_fog_start: f32,
    room_fog_end: f32,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

const TAG_INTERIOR_FOG: u32 = 0x40000000u;
const FRAMES_PER_SECOND: f32 = 24.0;
const SCROLL_PERIOD: f32 = 10.0;
const SWATCH_ROWS: f32 = 64.0;
const OCEAN_LAST_ROW_VALUE: f32 = 0.9;
const MIN_GLINT: f32 = 0.25;
const RENDERER_BUILDING_OUTDOORS: f32 = 1.0;
const RENDERER_BUILDING_INDOORS: f32 = 2.0;

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

// The lighting sun never sets, so the fixed-function N·L gate never closes on the flat up normal.
fn sun_sheen(world_normal: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let n = normalize(world_normal);
    let to_light = -normalize(wow_light.sun_travel.xyz);
    let to_view = normalize(view.world_position.xyz - world_pos);
    let half_v = normalize(to_light + to_view);
    let ndoth = max(dot(n, half_v), 0.0);
    return wow_light.light_spec.rgb * pow(ndoth, max(w.shininess, 1.0));
}

fn anim_time() -> f32 {
    return globals.time;
}

fn frame_layer() -> i32 {
    return i32(floor(anim_time() * FRAMES_PER_SECOND) % max(w.frame_count, 1.0));
}

fn apply_scroll(uv: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(uv.x, uv.y + w.scrolls * fract(anim_time() / SCROLL_PERIOD));
}

fn apply_fog(rgb: vec3<f32>, world_pos: vec3<f32>, room_fog: u32) -> vec3<f32> {
    var color = wow_light.fog_color;
    var on = wow_light.fog_on;
    var start = wow_light.fog_start;
    var end = wow_light.fog_end;
    if (w.room_fogged > 0.5 && room_fog != 0u) {
        color = wow_light.room_fog_color;
        on = wow_light.room_fog_on;
        start = wow_light.room_fog_start;
        end = wow_light.room_fog_end;
    }
    if (on <= 0.5) {
        return rgb;
    }
    let eye_z = -(view.view_from_world * vec4<f32>(world_pos, 1.0)).z;
    let denom = max(end - start, 0.001);
    let factor = clamp((end - eye_z) / denom, 0.0, 1.0);
    return mix(color, rgb, factor);
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

fn swatch_at(shallow: vec4<f32>, deep: vec4<f32>, v: f32, ocean: bool) -> vec4<f32> {
    let t = clamp(v * SWATCH_ROWS - 0.5, 0.0, SWATCH_ROWS - 1.0);
    let i0 = floor(t);
    return mix(
        swatch_row(shallow, deep, i0, ocean),
        swatch_row(shallow, deep, min(i0 + 1.0, SWATCH_ROWS - 1.0), ocean),
        t - i0,
    );
}

fn primary_light(world_normal: vec3<f32>) -> vec3<f32> {
    let n = normalize(world_normal);
    let to_light = -normalize(wow_light.sun_travel.xyz);
    return clamp(
        wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * max(dot(n, to_light), 0.0),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

fn fullbright_liquid(in: LiquidVsOut, sheet: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(apply_fog(sheet.rgb, in.world_position.xyz, in.room_fog), 1.0);
}

fn building_water_indoors(in: LiquidVsOut, sheet: vec4<f32>, alpha: f32) -> vec4<f32> {
    let body = clamp(in.vcolor.rgb + sheet.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(
        apply_fog(body, in.world_position.xyz, in.room_fog),
        clamp(alpha + sheet.a, 0.0, 1.0),
    );
}

fn building_water_outdoors(
    in: LiquidVsOut,
    sheet: vec4<f32>,
    deep: vec4<f32>,
    alpha: f32,
) -> vec4<f32> {
    let rgb = primary_light(in.world_normal) * deep.rgb + sheet.rgb + in.secondary_vtx * sheet.a;
    return vec4<f32>(apply_fog(rgb, in.world_position.xyz, in.room_fog), alpha);
}

fn terrain_water(in: LiquidVsOut, sheet: vec4<f32>, shallow: vec4<f32>, deep: vec4<f32>) -> vec4<f32> {
    let swatch = swatch_at(shallow, deep, clamp(in.depth, 0.0, 1.0), w.ocean > 0.5);
    let glint = in.secondary_vtx + vec3<f32>(MIN_GLINT);
    let rgb = primary_light(in.world_normal) * swatch.rgb + sheet.rgb + glint * sheet.a;
    return vec4<f32>(apply_fog(rgb, in.world_position.xyz, in.room_fog), swatch.w);
}

@fragment
fn fragment(in: LiquidVsOut) -> @location(0) vec4<f32> {
    if (wow_light.far_wall > 0.0) {
        let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        if (eye_z > wow_light.far_wall) {
            discard;
        }
    }
    let sheet = textureSampleBias(
        frames,
        frames_samp,
        apply_scroll(in.uv),
        frame_layer(),
        view.mip_bias,
    );
    if (w.fullbright > 0.5) {
        return fullbright_liquid(in, sheet);
    }
    var shallow = wow_light.river_shallow;
    var deep = wow_light.river_deep;
    if (w.ocean > 0.5) {
        shallow = wow_light.ocean_shallow;
        deep = wow_light.ocean_deep;
    }
    let alpha = mix(shallow.w, deep.w, clamp(in.depth, 0.0, 1.0));
    if (w.renderer >= RENDERER_BUILDING_INDOORS - 0.5) {
        return building_water_indoors(in, sheet, alpha);
    }
    if (w.renderer >= RENDERER_BUILDING_OUTDOORS - 0.5) {
        return building_water_outdoors(in, sheet, deep, alpha);
    }
    return terrain_water(in, sheet, shallow, deep);
}
