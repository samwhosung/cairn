#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::position_world_to_clip,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> colour: vec4<f32>;

struct MarkVsOut {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn vertex(in: Vertex) -> MarkVsOut {
    var out: MarkVsOut;
#ifdef MARK_IN_NDC
    out.clip_position = vec4<f32>(in.position.xy, 0.5, 1.0);
#else
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    let world = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(in.position, 1.0),
    );
    out.clip_position = position_world_to_clip(world.xyz);
#endif
    return out;
}

@fragment
fn fragment(in: MarkVsOut) -> @location(0) vec4<f32> {
    return colour;
}
