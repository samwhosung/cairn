// The client draws the WDL unlit and white, fogged with start 0 and end 1 yard, so it is flat fog
// colour, in a depth range behind the detailed world; with reverse-Z that range is a clamp just
// behind the wall's depth, still in front of the sky's.

#import bevy_pbr::{
    mesh_functions,
    forward_io::Vertex,
    view_transformations::{position_world_to_clip, view_z_to_depth_ndc},
    mesh_view_bindings::view,
}

// The client starts the WDL one coarse cell inside the far-clip wall, so no gap opens where coarse
// and fine part.
const WDL_OVERLAP: f32 = 33.0;

// Keeps the band strictly behind a detailed fragment exactly at the wall rather than tied with it.
const WDL_DEPTH_PUSH: f32 = 0.999;

struct WowLight {
    _light_ambient: vec4<f32>,
    _light_diffuse: vec4<f32>,
    _light_sun: vec4<f32>,
    _light_spec: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
};
@group(#{MATERIAL_BIND_GROUP}) @binding(90) var<storage, read> w: WowLight;

struct WdlVsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
}

struct WdlFsOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@vertex
fn vertex(in: Vertex) -> WdlVsOut {
    var out: WdlVsOut;
    let world_from_local = mesh_functions::get_world_from_local(in.instance_index);
    out.world_position =
        mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(in.position, 1.0));
    out.clip_position = position_world_to_clip(out.world_position.xyz);
    return out;
}

@fragment
fn fragment(in: WdlVsOut) -> WdlFsOut {
    let eye_z = -(view.view_from_world * vec4<f32>(in.world_position.xyz, 1.0)).z;
    let farclip = w.fog_params.w;

    if (farclip > 0.0 && eye_z < farclip - WDL_OVERLAP) {
        discard;
    }

    var rgb = vec3<f32>(1.0);
    if (w.fog_color.w > 0.5) {
        rgb = w.fog_color.xyz;
    }

    var out: WdlFsOut;
    out.color = vec4<f32>(rgb, 1.0);
    var depth = in.clip_position.z;
    if (farclip > 0.0) {
        depth = min(depth, view_z_to_depth_ndc(-farclip) * WDL_DEPTH_PUSH);
    }
    out.depth = depth;
    return out;
}
