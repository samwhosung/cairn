// The client's taps sit whole texels away from a point half a texel up and left of each target
// texel's centre. The float frame does not saturate per draw as the client's byte one does, so
// every read of it clamps to 1 first.

#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var in_tex: texture_2d<f32>;
@group(0) @binding(1) var in_samp: sampler;
@group(0) @binding(2) var blur_tex: texture_2d<f32>;
struct Glow {
    weight: f32,
}
@group(0) @binding(3) var<uniform> glow: Glow;

const HALF_TEXEL: f32 = 0.5;
const DOWNSAMPLE_TAPS: array<vec2<f32>, 4> = array<vec2<f32>, 4>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, 1.0),
);
const BLUR_TAPS: array<f32, 4> = array<f32, 4>(-2.0, 0.0, 1.0, 3.0);
const BLUR_WEIGHTS: array<f32, 4> = array<f32, 4>(0.125, 0.375, 0.375, 0.125);

@fragment
fn downsample(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(in_tex));
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let at = in.uv + (DOWNSAMPLE_TAPS[i] - HALF_TEXEL) * texel;
        sum += min(textureSample(in_tex, in_samp, at), vec4<f32>(1.0));
    }
    return sum * 0.25;
}

fn blur(uv: vec2<f32>, axis: vec2<f32>) -> vec4<f32> {
    let texel = axis / vec2<f32>(textureDimensions(in_tex));
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let at = uv + (BLUR_TAPS[i] - HALF_TEXEL) * texel;
        sum += textureSample(in_tex, in_samp, at) * BLUR_WEIGHTS[i];
    }
    return sum;
}

@fragment
fn blur_across(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(1.0, 0.0));
}

@fragment
fn blur_down(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    return blur(in.uv, vec2<f32>(0.0, 1.0));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn combine(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let frame = clamp(textureLoad(in_tex, vec2<i32>(in.position.xy), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let blurred = max(textureSample(blur_tex, in_samp, in.uv).rgb, vec3<f32>(0.0));
    let glowed = min(frame + glow.weight * blurred * blurred, vec3<f32>(1.0));
    return vec4<f32>(srgb_to_linear(glowed), 1.0);
}
