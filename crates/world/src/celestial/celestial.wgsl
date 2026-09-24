// A disc is clipped at the horizon per vertex: a vertex below it takes alpha 0, one within a
// 0.4-yard band above it (on the client's 12-yard sphere) takes the band's ramp, one above keeps
// the colour's alpha, and the rasterizer interpolates.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    forward_io::VertexOutput,
    mesh_view_bindings::view,
}

struct SpriteLook {
    horizon_slope: f32,
    glare: u32,
    disc_alpha: f32,
}

struct DiscSpan {
    sin_bottom: f32,
    sin_top: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> look: SpriteLook;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<uniform> span: DiscSpan;

fn vertex_alpha(y: f32) -> f32 {
    let ramp = y * look.horizon_slope;
    return select(look.disc_alpha, clamp(ramp, 0.0, 1.0), ramp < 1.0);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let base = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var a = base.a;
    if look.glare == 0u {
        let y = normalize(in.world_position.xyz - view.world_position.xyz).y;
        if y <= 0.0 {
            a = 0.0;
        } else {
            let y_bot = max(span.sin_bottom, 0.0);
            let t = clamp((y - y_bot) / max(span.sin_top - y_bot, 1e-6), 0.0, 1.0);
            a *= mix(vertex_alpha(y_bot), vertex_alpha(span.sin_top), t);
        }
    }

    // The texture is sRGB, so its texels arrive linear; the target takes gamma values.
    let gamma = linear_to_srgb(base.rgb);
    if look.glare != 0u {
        return vec4<f32>(gamma * a, 0.0);
    }
    return vec4<f32>(gamma * a, a);
}
