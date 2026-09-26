// M2 and WMO batches lit as the client lights them, in gamma space.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    pbr_bindings,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
    mesh_functions,
}

struct WowFragOut {
    @location(0) color: vec4<f32>,
}

struct ModelParams {
    clutter_fade: vec4<f32>,
    model_flags: vec4<f32>,
    sun_scale: vec4<f32>,
    tint: vec4<f32>,
    sidn: vec4<f32>,
    anim_slots: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> m: ModelParams;

struct WowLight {
    light_ambient: vec4<f32>,
    light_diffuse: vec4<f32>,
    light_sun: vec4<f32>,
    light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    sh_c10_r: vec4<f32>,
    sh_c10_g: vec4<f32>,
    sh_c10_b: vec4<f32>,
    sh_c13_r: vec4<f32>,
    sh_c13_g: vec4<f32>,
    sh_c13_b: vec4<f32>,
    sh_c16: vec4<f32>,
    _water: array<vec4<f32>, 4>,
    grade: vec4<f32>,
    wmo_fog_color: vec4<f32>,
    wmo_fog_params: vec4<f32>,
    point_count: vec4<f32>,
    points: array<vec4<f32>, 512>,
    prop_probes: array<vec4<f32>, 57344>,
    rig_table: array<u32, 2048>,
    rig_tint: array<u32, 2048>,
    rig_origin: array<vec4<f32>, 2048>,
    matanim: array<vec4<f32>, 2048>,
    palettes: array<vec4<f32>>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> wow_light: WowLight;

const VANILLA_ALPHA_KEY: f32 = 0.8784314;
const DETAIL_DOODAD_MIP_BIAS: f32 = 0.25;
const F32_EPSILON: f32 = 1.1920929e-7;
const SKY_FAR_CLIP_Z: f32 = 0.0;
const TILE_YARDS: f32 = 533.33333;

const NEAREST_POINT_LIGHTS: u32 = 3u;
const POINT_FALLOFF_LINEAR: f32 = 0.7;
const POINT_FALLOFF_QUADRATIC: f32 = 0.03;

const ADDITIVE_BIT: u32 = 4u;
const OPAQUE_INTENT_BIT: u32 = 8u;
const FOG_POLICY_SHIFT: u32 = 4u;
const MODULATE_BIT: u32 = 128u;
const MODULATE_2X_BIT: u32 = 256u;
const TWIN_CUTOUT_BIT: u32 = 1024u;
const ENV_MAP_BIT: u32 = 4096u;

const FOG_BLACK: u32 = 1u;
const FOG_WHITE: u32 = 2u;
const FOG_GREY: u32 = 3u;
const FOG_OFF: u32 = 4u;

const TAG_ALPHA_MASK: u32 = 0x3fu;
const TAG_SHADE_SHIFT: u32 = 6u;
const TAG_PROBE_MASK: u32 = 0x1fffu;
const TAG_SLOT_SHIFT: u32 = 19u;
const TAG_SLOT_MASK: u32 = 0x7ffu;
const BONE_ROWS: u32 = 3u;
const TAG_INTERIOR_FOG: u32 = 0x40000000u;
const TAG_HIGHLIGHT: u32 = 0x80000000u;

fn markers() -> u32 {
    return u32(m.clutter_fade.z);
}

fn has_marker(bit: u32) -> bool {
    return (markers() & bit) != 0u;
}

fn is_wmo() -> bool {
    return m.model_flags.x > 0.5;
}

fn is_interior() -> bool {
    return m.model_flags.z > 0.5;
}

fn is_clutter() -> bool {
    return m.clutter_fade.w > 0.5;
}

fn wmo_class_ext() -> bool {
    return m.tint.w < 0.5;
}

fn wmo_class_int() -> bool {
    return m.tint.w > 0.5 && m.tint.w < 1.5;
}

fn wmo_class_trans() -> bool {
    return m.tint.w > 1.5;
}

struct WowVsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    @location(6) @interpolate(flat) instance_index: u32,
#endif
    @location(8) point_lit: vec3<f32>,
}

// A zero normal stays zero, which lights at the SH lobe's DC term as the client does.
fn wow_normalize(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    return select(vec3<f32>(0.0), normalize(v), l2 > 1e-12);
}

fn point_light_sum(P: vec3<f32>, N: vec3<f32>, anchor: vec3<f32>) -> vec3<f32> {
    let count = u32(wow_light.point_count.x);
    var sel = array<u32, NEAREST_POINT_LIGHTS>(0u, 0u, 0u);
    var sd = array<f32, NEAREST_POINT_LIGHTS>(1e30, 1e30, 1e30);
    for (var i = 0u; i < count; i = i + 1u) {
        let pos_range = wow_light.points[2u * i];
        let dv = pos_range.xyz - anchor;
        let d2 = dot(dv, dv);
        if (d2 > pos_range.w * pos_range.w) {
            continue;
        }
        if (d2 < sd[0]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = sd[0]; sel[1] = sel[0];
            sd[0] = d2; sel[0] = i;
        } else if (d2 < sd[1]) {
            sd[2] = sd[1]; sel[2] = sel[1];
            sd[1] = d2; sel[1] = i;
        } else if (d2 < sd[2]) {
            sd[2] = d2; sel[2] = i;
        }
    }
    var sum = vec3<f32>(0.0);
    for (var s = 0u; s < NEAREST_POINT_LIGHTS; s = s + 1u) {
        if (sd[s] > 9.9e29) {
            break;
        }
        let pos_range = wow_light.points[2u * sel[s]];
        let to_light = pos_range.xyz - P;
        let d = length(to_light);
        let atten = 1.0 / (POINT_FALLOFF_LINEAR * d + POINT_FALLOFF_QUADRATIC * d * d);
        let nl = max(dot(N, to_light / max(d, 1e-4)), 0.0);
        sum += wow_light.points[2u * sel[s] + 1u].rgb * (atten * nl);
    }
    return sum;
}

fn mcnk_cell_anchor(P: vec3<f32>) -> vec3<f32> {
    let cell = TILE_YARDS / 16.0;
    let half = 32.0 * TILE_YARDS;
    let ix = floor((half + P.x) / cell);
    let iz = floor((half + P.z) / cell);
    return vec3<f32>((ix + 0.5) * cell - half, P.y, (iz + 0.5) * cell - half);
}

struct WowVertex {
    @builtin(instance_index) instance_index: u32,
#ifdef VERTEX_POSITIONS
    @location(0) position: vec3<f32>,
#endif
#ifdef VERTEX_NORMALS
    @location(1) normal: vec3<f32>,
#endif
#ifdef VERTEX_UVS_A
    @location(2) uv: vec2<f32>,
#endif
#ifdef VERTEX_UVS_B
    @location(3) uv_b: vec2<f32>,
#endif
#ifdef VERTEX_COLORS
    @location(5) color: vec4<f32>,
#endif
#ifdef WOW_RIG_SKIN
    @location(10) joint_indices: vec4<u32>,
    @location(11) joint_weights: vec4<f32>,
#endif
}

#ifdef WOW_RIG_SKIN
fn wow_rig_slot(instance_index: u32) -> u32 {
    return (mesh_functions::get_tag(instance_index) >> TAG_SLOT_SHIFT) & TAG_SLOT_MASK;
}

fn wow_skin_model(instance_index: u32, indices: vec4<u32>, weights: vec4<f32>) -> mat4x4<f32> {
    let base = wow_light.rig_table[wow_rig_slot(instance_index)];
    let b0 = BONE_ROWS * (base + indices.x);
    let b1 = BONE_ROWS * (base + indices.y);
    let b2 = BONE_ROWS * (base + indices.z);
    let b3 = BONE_ROWS * (base + indices.w);
    let r0 = weights.x * wow_light.palettes[b0]
        + weights.y * wow_light.palettes[b1]
        + weights.z * wow_light.palettes[b2]
        + weights.w * wow_light.palettes[b3];
    let r1 = weights.x * wow_light.palettes[b0 + 1u]
        + weights.y * wow_light.palettes[b1 + 1u]
        + weights.z * wow_light.palettes[b2 + 1u]
        + weights.w * wow_light.palettes[b3 + 1u];
    let r2 = weights.x * wow_light.palettes[b0 + 2u]
        + weights.y * wow_light.palettes[b1 + 2u]
        + weights.z * wow_light.palettes[b2 + 2u]
        + weights.w * wow_light.palettes[b3 + 2u];
    return mat4x4<f32>(
        vec4<f32>(r0.x, r1.x, r2.x, 0.0),
        vec4<f32>(r0.y, r1.y, r2.y, 0.0),
        vec4<f32>(r0.z, r1.z, r2.z, 0.0),
        vec4<f32>(r0.w, r1.w, r2.w, 1.0),
    );
}

fn inverse_transpose_3x3m(in: mat3x3<f32>) -> mat3x3<f32> {
    let x = cross(in[1], in[2]);
    let y = cross(in[2], in[0]);
    let z = cross(in[0], in[1]);
    let det = dot(in[2], z);
    return mat3x3<f32>(x / det, y / det, z / det);
}

fn wow_skin_normals(frame_from_local: mat4x4<f32>, normal: vec3<f32>) -> vec3<f32> {
    return wow_normalize(
        inverse_transpose_3x3m(mat3x3<f32>(
            frame_from_local[0].xyz,
            frame_from_local[1].xyz,
            frame_from_local[2].xyz
        )) * normal
    );
}
#endif

@vertex
fn vertex(vertex: WowVertex) -> WowVsOut {
    var out: WowVsOut;

    // The placement splits into its rotation and its origin, so no vertex is a big-times-small
    // product in f32.
    let mesh_world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
#ifdef WOW_RIG_SKIN
    var frame_from_local = wow_skin_model(
        vertex.instance_index,
        vertex.joint_indices,
        vertex.joint_weights
    );
    let frame_origin = wow_light.rig_origin[wow_rig_slot(vertex.instance_index)].xyz;
#else
    var frame_from_local = mesh_world_from_local;
    let frame_origin = mesh_world_from_local[3].xyz;
    frame_from_local[3] = vec4<f32>(0.0, 0.0, 0.0, 1.0);
#endif

#ifdef VERTEX_NORMALS
#ifdef WOW_RIG_SKIN
    out.world_normal = wow_skin_normals(frame_from_local, vertex.normal);
#else
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index
    );
#endif
#endif

#ifdef VERTEX_POSITIONS
    let p_cam = (frame_from_local * vec4<f32>(vertex.position, 1.0)).xyz
        + (frame_origin - view.world_position);
    out.world_position = vec4<f32>(p_cam + view.world_position, 1.0);
    let view_rot = mat3x3<f32>(
        view.view_from_world[0].xyz,
        view.view_from_world[1].xyz,
        view.view_from_world[2].xyz,
    );
    out.position = view.clip_from_view * vec4<f32>(view_rot * p_cam, 1.0);
    // A later WMO batch wins its coplanar tie by n ULPs of depth, as the client's draw order does.
    out.position.z *= 1.0 + m.sun_scale.y * F32_EPSILON;
#ifdef WOW_SKY_DEPTH
    out.position.z = SKY_FAR_CLIP_Z;
#endif
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#ifdef VERTEX_POSITIONS
#ifdef VERTEX_NORMALS
    if (has_marker(ENV_MAP_BIT)) {
        out.uv = env_map_uv(view_rot * p_cam, normalize(view_rot * out.world_normal));
    }
#endif
#endif
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

    if (is_wmo() || is_interior()) {
        out.point_lit = vec3<f32>(0.0);
    } else {
        var anchor = mesh_world_from_local[3].xyz;
        if (is_clutter()) {
            anchor = mcnk_cell_anchor(out.world_position.xyz);
        }
        out.point_lit = point_light_sum(out.world_position.xyz, out.world_normal, anchor);
    }
    return out;
}

#ifdef WOW_SIGHT
// A blended batch covers a pixel where it gives at least half its colour; one that adds or
// multiplies covers none, since what lies behind it still shows.
const SIGHT_MIN_ALPHA: f32 = 0.5;

// The placement's index, from the tag's shade and fog bits, as three sRGB bytes of 6, 6 and 3
// bits, each 4n + 2, so a byte a rounding off still reads.
fn sight_colour(tag: u32) -> vec3<f32> {
    let index = ((tag >> 6u) & 0x1fffu) | ((tag >> 30u) << 13u);
    let code = vec3<u32>(index & 63u, (index >> 6u) & 63u, index >> 12u);
    let s = (vec3<f32>(code) * 4.0 + 2.0) / 255.0;
    return select(pow((s + 0.055) / 1.055, vec3<f32>(2.4)), s / 12.92, s <= vec3<f32>(0.04045));
}
#endif

fn env_map_uv(p_view: vec3<f32>, n_view: vec3<f32>) -> vec2<f32> {
    let refl = normalize(p_view - 2.0 * dot(p_view, n_view) * n_view);
    return refl.xy * 0.5 + vec2<f32>(0.5, 0.5);
}

@fragment
fn fragment(in: WowVsOut, @builtin(front_facing) is_front: bool) -> WowFragOut {
    if (m.anim_slots.w > 0.5) {
        let r = wow_light.matanim[u32(m.anim_slots.w)];
        if (in.position.x < r.x || in.position.y < r.y
            || in.position.x > r.z || in.position.y > r.w) {
            discard;
        }
    }
    if (wow_light.fog_params.w > 0.0) {
        let clip_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        if (clip_z > wow_light.fog_params.w) {
            discard;
        }
    }
    var vo: VertexOutput;
    vo.position = in.position;
    vo.world_position = in.world_position;
    vo.world_normal = in.world_normal;
#ifdef VERTEX_UVS_A
    if (has_marker(ENV_MAP_BIT)) {
        vo.uv = in.uv;
    } else {
        let uv_t = in.uv + m.sun_scale.zw + wow_light.matanim[u32(m.anim_slots.x)].xy;
        let affine = wow_light.matanim[u32(m.anim_slots.z)];
        let d = (uv_t - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.0 + affine.z, 1.0 + affine.w);
        let c = 1.0 + affine.x;
        vo.uv = vec2<f32>(0.5 + d.x * c - d.y * affine.y, 0.5 + d.x * affine.y + d.y * c);
    }
#endif
#ifdef VERTEX_UVS_B
    vo.uv_b = in.uv_b;
#endif
#ifdef VERTEX_COLORS
    vo.color = in.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    vo.instance_index = in.instance_index;
#endif
    var pbr_input = pbr_input_from_standard_material(vo, is_front);
    var base_color = pbr_input.material.base_color;
    // A WMO's MOCV alpha carries lighting, never coverage: coverage is the texel's own alpha.
#ifdef VERTEX_COLORS
    if (is_wmo()) {
#ifdef VERTEX_UVS_A
        base_color.a = textureSampleBias(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            vo.uv,
            view.mip_bias,
        ).a;
#else
        base_color.a = 1.0;
#endif
    }
#endif
    if (is_clutter()) {
#ifdef VERTEX_UVS_A
        var biased = textureSampleBias(
            pbr_bindings::base_color_texture,
            pbr_bindings::base_color_sampler,
            vo.uv,
            view.mip_bias + DETAIL_DOODAD_MIP_BIAS,
        );
#ifdef VERTEX_COLORS
        biased = biased * vo.color;
#endif
        base_color = biased;
#endif
    }
    // Clutter fades by view depth through the client's quantised 64-texel ramp.
    if (is_clutter()) {
        let z_eye = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        let u = (z_eye - m.clutter_fade.x) / max(m.clutter_fade.y - m.clutter_fade.x, 0.001);
        let ramp = clamp((254.0 - 256.0 * u) / 255.0, 0.0, 252.0 / 255.0);
        base_color.a = base_color.a * ramp;
    }

    let interior_prop = is_interior() && !is_wmo();
    let raw_tag = mesh_functions::get_tag(in.instance_index);
    let highlighted = (raw_tag & TAG_HIGHLIGHT) != 0u;
    let interior_fogged = (raw_tag & TAG_INTERIOR_FOG) != 0u;
    let fade_tag = raw_tag & ~(TAG_HIGHLIGHT | TAG_INTERIOR_FOG);
    let alpha6 = f32(fade_tag & TAG_ALPHA_MASK) / 63.0;
    var obj_fade = select(alpha6, 1.0, fade_tag == 0u);
    let tint_word = wow_light.rig_tint[(fade_tag >> TAG_SLOT_SHIFT) & TAG_SLOT_MASK];
    let inst_tint = select(
        vec3<f32>(
            f32((tint_word >> 16u) & 0xffu),
            f32((tint_word >> 8u) & 0xffu),
            f32(tint_word & 0xffu),
        ) * (1.0 / 255.0),
        vec3<f32>(1.0),
        tint_word == 0u,
    );
    if (has_marker(TWIN_CUTOUT_BIT) && base_color.a < VANILLA_ALPHA_KEY) {
        discard;
    }
    let faded_alpha = base_color.a * obj_fade;
    let base = alpha_discard(pbr_input.material, base_color);
#ifdef WOW_SIGHT
    let lights_or_shades = has_marker(ADDITIVE_BIT) || has_marker(MODULATE_BIT)
        || has_marker(MODULATE_2X_BIT);
    if (lights_or_shades || (!has_marker(OPAQUE_INTENT_BIT) && faded_alpha < SIGHT_MIN_ALPHA)) {
        discard;
    }
#endif

    let L = -normalize(wow_light.light_sun.xyz);
    let n_m2 = wow_normalize(pbr_input.world_normal);
    // Both faces of a two-sided batch light from the submitted normal: the client never enables
    // two-sided lighting.
    let n_lit = select(-n_m2, n_m2, is_front);
    let ndotl = max(dot(n_lit, L), 0.0);
    let lit_nl = clamp(wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb * ndotl, vec3<f32>(0.0), vec3<f32>(1.0));
    let quad = vec4<f32>(n_lit.x * n_lit.y, n_lit.y * n_lit.z, n_lit.z * n_lit.z, n_lit.x * n_lit.z);
    let n1 = vec4<f32>(n_lit, 1.0);
    let x2y2 = n_lit.x * n_lit.x - n_lit.y * n_lit.y;
    let inst_shade = select(f32((fade_tag >> TAG_SHADE_SHIFT) & 0xffu) / 255.0, 0.0, interior_prop);
    let mat_shade = select(0.0, 1.0, m.sun_scale.x < 0.5);
    let shade_t = max(mat_shade, inst_shade);
    let mid_band = m.sun_scale.x >= 0.5 && m.sun_scale.x < 0.85;
    let intensity = min(select(mix(2.5, 0.5, shade_t), 1.0, mid_band), 1.0);
    let sun_dc = wow_light.grade.yzw * intensity;
    let sun_lobe = vec3<f32>(
        wow_light.sh_c10_r.w + sun_dc.x
            + intensity
                * (dot(wow_light.sh_c10_r.xyz, n_lit) + dot(wow_light.sh_c13_r, quad)
                    + wow_light.sh_c16.x * x2y2),
        wow_light.sh_c10_g.w + sun_dc.y
            + intensity
                * (dot(wow_light.sh_c10_g.xyz, n_lit) + dot(wow_light.sh_c13_g, quad)
                    + wow_light.sh_c16.y * x2y2),
        wow_light.sh_c10_b.w + sun_dc.z
            + intensity
                * (dot(wow_light.sh_c10_b.xyz, n_lit) + dot(wow_light.sh_c13_b, quad)
                    + wow_light.sh_c16.z * x2y2),
    );
    // Clamped as a sum: the lobe dips below ambient mid-back, and that dip is the client's.
    let lit_doodad = clamp(sun_lobe, vec3<f32>(0.0), vec3<f32>(1.0));
    let use_doodad_shade = (wow_light.light_sun.w > 0.5) && !is_clutter() && !is_wmo();
    let lit_exterior = select(lit_nl, lit_doodad, use_doodad_shade);
    var trans_a = 1.0;
#ifdef VERTEX_COLORS
    trans_a = in.color.a;
#endif
    let window_mid = 0.5 * (wow_light.light_ambient.rgb + wow_light.light_diffuse.rgb);
    let lit_window = clamp(
        window_mid + vec3<f32>(16.0 / 255.0) + window_mid * ndotl,
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let lit_int_base = select(lit_nl, lit_window, m.sidn.w > 0.5);
    var lit_wmo_interior = vec3<f32>(1.0);
    if (wmo_class_trans()) {
        lit_wmo_interior = mix(vec3<f32>(1.0), lit_int_base, trans_a);
    } else if (wmo_class_ext()) {
        lit_wmo_interior = lit_int_base;
    }
    let probe = 7u * ((fade_tag >> TAG_SHADE_SHIFT) & TAG_PROBE_MASK);
    let lit_m2_interior = clamp(
        vec3<f32>(
            dot(wow_light.prop_probes[probe + 0u], n1)
                + dot(wow_light.prop_probes[probe + 3u], quad)
                + wow_light.prop_probes[probe + 6u].x * x2y2,
            dot(wow_light.prop_probes[probe + 1u], n1)
                + dot(wow_light.prop_probes[probe + 4u], quad)
                + wow_light.prop_probes[probe + 6u].y * x2y2,
            dot(wow_light.prop_probes[probe + 2u], n1)
                + dot(wow_light.prop_probes[probe + 5u], quad)
                + wow_light.prop_probes[probe + 6u].z * x2y2,
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    let lit_interior = select(lit_m2_interior, lit_wmo_interior, is_wmo());
    let is_rig = m.sun_scale.x >= 1.5;
    let lit = select(select(lit_exterior, lit_interior, is_interior()), lit_m2_interior, is_rig);
    let anim_tint = m.tint.rgb + wow_light.matanim[u32(m.anim_slots.y)].xyz;
    let albedo = base.rgb * anim_tint;
    let is_emissive = m.model_flags.w > 0.5;
    var sidn_w = 1.0;
    if (is_interior() && is_wmo()) {
        if (wmo_class_trans()) {
            sidn_w = trans_a;
        } else if (wmo_class_int()) {
            sidn_w = 0.0;
        }
    }
    let sidn_e = m.sidn.rgb * (wow_light.grade.x * sidn_w);
    let point_diffuse = in.point_lit;
    let highlight = select(0.0, 0.2509804, highlighted);
    var lit_rgb: vec3<f32>;
#ifdef VERTEX_COLORS
    if (is_wmo()) {
        let vc = in.color.rgb;
        let primary = clamp(
            vc * (lit + point_diffuse) + sidn_e + vec3<f32>(highlight),
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        let tex_rgb = base.rgb / max(vc, vec3<f32>(1.0 / 255.0));
        lit_rgb = tex_rgb * m.tint.rgb * primary;
        if (is_interior() && wmo_class_int()) {
            lit_rgb = clamp(
                tex_rgb * m.tint.rgb * vc * (1.0 + 4.0 * trans_a),
                vec3<f32>(0.0),
                vec3<f32>(1.0),
            );
        }
    } else {
        let primary = clamp(
            inst_tint * (lit + point_diffuse) + sidn_e + vec3<f32>(highlight),
            vec3<f32>(0.0),
            vec3<f32>(1.0),
        );
        lit_rgb = albedo * primary;
    }
#else
    let primary = clamp(
        inst_tint * (lit + point_diffuse) + sidn_e + vec3<f32>(highlight),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    lit_rgb = albedo * primary;
#endif
    var unlit_rgb = albedo * inst_tint;
    if (!is_wmo()) {
#ifdef VERTEX_COLORS
        let m2_color = in.color.rgb * anim_tint;
        let texel = base.rgb / max(in.color.rgb, vec3<f32>(1.0 / 255.0));
#else
        let m2_color = anim_tint;
        let texel = base.rgb;
#endif
        // The client clamps an unlit M2's colour before the texel modulates it.
        unlit_rgb = texel * clamp(m2_color * inst_tint, vec3<f32>(0.0), vec3<f32>(1.0));
    }
    var rgb = select(lit_rgb, unlit_rgb, is_emissive);
    let is_mod = has_marker(MODULATE_BIT);
    let is_mod2x = has_marker(MODULATE_2X_BIT);
    // The client draws an M2 Mod or Mod2x batch from the bare texel.
    if ((is_mod || is_mod2x) && !is_wmo()) {
        rgb = base.rgb;
    }

    var fog_color = wow_light.fog_color;
    var fog_span = wow_light.fog_params.xy;
    if (interior_fogged) {
        fog_color = wow_light.wmo_fog_color;
        fog_span = wow_light.wmo_fog_params.xy;
    }
    let fog_policy = (markers() >> FOG_POLICY_SHIFT) & 7u;
    if (fog_color.w > 0.5 && fog_policy != FOG_OFF) {
        let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
        let denom = max(fog_span.y - fog_span.x, 0.001);
        let factor = clamp((fog_span.y - eye_z) / denom, 0.0, 1.0);
        var fog_rgb = fog_color.xyz;
        if (fog_policy == FOG_BLACK) { fog_rgb = vec3<f32>(0.0); }
        else if (fog_policy == FOG_WHITE) { fog_rgb = vec3<f32>(1.0); }
        else if (fog_policy == FOG_GREY) { fog_rgb = vec3<f32>(0.50196078); }
        rgb = mix(fog_rgb, rgb, factor);
    }

    var out: WowFragOut;
    let opaque_intent = has_marker(OPAQUE_INTENT_BIT);
    var out_rgb = rgb;
    if (has_marker(ADDITIVE_BIT)) {
        out_rgb = out_rgb * faded_alpha;
    }
    if (is_mod || is_mod2x) {
        let identity = select(vec3<f32>(1.0), vec3<f32>(0.5), is_mod2x);
        out_rgb = mix(identity, out_rgb, obj_fade);
    }
    out.color = vec4<f32>(out_rgb, select(faded_alpha, 1.0, opaque_intent));
#ifdef WOW_SIGHT
    out.color = vec4<f32>(sight_colour(raw_tag), 1.0);
#endif
    return out;
}
