#import bevy_pbr::forward_io::VertexOutput

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var field_bytes: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var cloud_samp: sampler;

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(field_bytes, cloud_samp, in.uv);
    var a = texel.a;
#ifdef VERTEX_COLORS
    a *= in.color.a;
#endif
    return vec4<f32>(texel.rgb * a, a);
}
