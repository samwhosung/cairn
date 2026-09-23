#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var grass: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var grass_sampler: sampler;

const CHUNK_YARDS: f32 = 100.0 / 3.0;
const REPEATS_PER_CHUNK: f32 = 8.0;
// 60 degrees up, in the south.
const TOWARD_SUN: vec3<f32> = vec3<f32>(0.0, 0.8660254, 0.5);
const AMBIENT: f32 = 0.5;
const DIFFUSE: f32 = 0.55;

// Texels are gamma-space bytes and WoW lights them as they are. The target is sRGB, so the lit
// color is decoded once here and encoded back to the same bytes when written.
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.world_position.xz * REPEATS_PER_CHUNK / CHUNK_YARDS;
    let texel = textureSample(grass, grass_sampler, uv).rgb;
    let light = AMBIENT + DIFFUSE * max(dot(normalize(in.world_normal), TOWARD_SUN), 0.0);
    return vec4<f32>(srgb_to_linear(saturate(texel * light)), 1.0);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let curve = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(curve, c / 12.92, c <= vec3<f32>(0.04045));
}
