#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

@group(0) @binding(0) var frame: texture_2d<f32>;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let gamma = textureLoad(frame, vec2<i32>(in.position.xy), 0).rgb;
    return vec4<f32>(srgb_to_linear(clamp(gamma, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}
