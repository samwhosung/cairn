#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    forward_io::VertexOutput,
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let a = pbr_input_from_standard_material(in, is_front).material.base_color.a;
    return vec4<f32>(vec3<f32>(a), a);
}
